//! The region layer's tests: the topology, the simplification (no gap, no overlap, no crossing at
//! any zoom), the tiles, the regions at a point. The step's own (keys, the real sample) are
//! terrain::regionlayer's.

use super::simplify::{meet, simplify, Simplified};
use super::topo::{build, Topology};
use super::*;
use std::collections::HashMap;

/// E7 of degrees.
fn e7(lon: f64, lat: f64) -> [i32; 2] {
    [(lon * 1e7).round() as i32, (lat * 1e7).round() as i32]
}

fn region(id: &str, folders: &[&str], rings: Vec<Vec<[i32; 2]>>) -> Region {
    Region { id: id.into(), folders: folders.iter().map(|s| s.to_string()).collect(), rings }
}

/// A box's ring, counter-clockwise (its inside on the left).
fn boxed(w: f64, s: f64, e: f64, n: f64) -> Vec<[i32; 2]> {
    vec![e7(w, s), e7(e, s), e7(e, n), e7(w, n)]
}

fn rev(mut r: Vec<[i32; 2]>) -> Vec<[i32; 2]> {
    r.reverse();
    r
}

/// A border from (x, s) to (x, n) wandering east and west (`amp` degrees), `k` points between.
fn wiggle(x: f64, s: f64, n: f64, amp: f64, k: usize, seed: u64) -> Vec<[i32; 2]> {
    let mut st = seed;
    let mut rnd = || {
        st = st.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((st >> 33) as f64 / (1u64 << 31) as f64) * 2.0 - 1.0
    };
    let mut v = vec![e7(x, s)];
    let mut off = 0.0;
    for i in 1..=k {
        off = (off + rnd() * amp * 0.5).clamp(-amp, amp);
        v.push(e7(x + off, s + (n - s) * i as f64 / (k + 1) as f64));
    }
    v.push(e7(x, n));
    v
}

/// The two sides of a border `b` (south to north) within a box: west of it and east of it, each
/// counter-clockwise.
fn split_box(w: f64, s: f64, e: f64, n: f64, b: &[[i32; 2]]) -> (Vec<[i32; 2]>, Vec<[i32; 2]>) {
    let mut west = vec![e7(w, s)];
    west.extend(b.iter().copied());
    west.push(e7(w, n));
    let mut east = vec![e7(e, s), e7(e, n)];
    east.extend(b.iter().rev().copied());
    (west, east)
}

fn run(regions: &[Region]) -> (Topology, Simplified, HashMap<(u8, u32, u32), Vec<u8>>) {
    let topo = build(regions);
    let s = simplify(&topo, &|_, _| {});
    let tiles = std::sync::Mutex::new(HashMap::new());
    tiles::all(&topo, &s, &|(z, x, y, b)| {
        tiles.lock().unwrap().insert((z, x, y), b);
    }, &|_| {});
    (topo, s, tiles.into_inner().unwrap())
}

fn has(topo: &Topology, set: u32, id: &str) -> bool {
    topo.sets[set as usize].iter().any(|&i| topo.ids[i as usize] == id)
}

/// Each pixel's regions (by index) at zoom `z`: even–odd over the arcs each has on one side, at
/// the pixels' centres of a `w` × `w` grid over `bx` (world units), and the same over the arcs
/// with one side empty (the union's outline).
fn raster(topo: &Topology, s: &Simplified, z: u8, bx: [f64; 4], w: usize) -> (Vec<u32>, Vec<bool>) {
    let segs_of = |pick: &dyn Fn(&topo::Arc) -> bool| -> Vec<([f64; 2], [f64; 2])> {
        let mut v = Vec::new();
        for (a, l) in topo.arcs.iter().zip(&s.lines) {
            if !pick(a) {
                continue;
            }
            let k: Vec<usize> = l.kept(z).collect();
            for p in k.windows(2) {
                v.push((l.pts[p[0]], l.pts[p[1]]));
            }
        }
        v
    };
    let fill = |segs: &[([f64; 2], [f64; 2])]| -> Vec<bool> {
        let mut out = vec![false; w * w];
        for row in 0..w {
            let yc = bx[1] + (bx[3] - bx[1]) * (row as f64 + 0.5) / w as f64;
            let mut xs: Vec<f64> = segs
                .iter()
                .filter(|(a, b)| (a[1] > yc) != (b[1] > yc))
                .map(|(a, b)| a[0] + (b[0] - a[0]) * (yc - a[1]) / (b[1] - a[1]))
                .collect();
            xs.sort_by(f64::total_cmp);
            for col in 0..w {
                let xc = bx[0] + (bx[2] - bx[0]) * (col as f64 + 0.5) / w as f64;
                out[row * w + col] = xs.iter().filter(|&&x| x < xc).count() % 2 == 1;
            }
        }
        out
    };
    let mut cover = vec![0u32; w * w];
    for k in 0..topo.ids.len() as u32 {
        let segs = segs_of(&|a| topo.sets[a.l as usize].contains(&k) != topo.sets[a.r as usize].contains(&k));
        for (c, i) in cover.iter_mut().zip(fill(&segs)) {
            *c += i as u32;
        }
    }
    let outline = fill(&segs_of(&|a| (a.l == 0) != (a.r == 0)));
    (cover, outline)
}

/// Every zoom: no pixel in two regions, and the regions' pixels exactly those inside their union's
/// outline (no gap); no two drawn lines meeting but at their shared ends.
fn check_every_zoom(topo: &Topology, s: &Simplified, bx: [f64; 4]) {
    for z in 0..=MAXZ {
        let (cover, outline) = raster(topo, s, z, bx, 300);
        let over = cover.iter().filter(|&&c| c > 1).count();
        assert_eq!(over, 0, "zoom {z}: {over} pixels in two regions");
        let gaps = cover.iter().zip(&outline).filter(|(c, o)| (**c == 0) == **o).count();
        assert_eq!(gaps, 0, "zoom {z}: {gaps} pixels in no region inside the regions' outline, or one outside it");
        let segs: Vec<([f64; 2], [f64; 2], usize)> = s
            .lines
            .iter()
            .enumerate()
            .flat_map(|(k, l)| {
                let kept: Vec<usize> = l.kept(z).collect();
                kept.windows(2).map(|p| (l.pts[p[0]], l.pts[p[1]], k)).collect::<Vec<_>>()
            })
            .collect();
        for i in 0..segs.len() {
            for j in i + 1..segs.len() {
                let ((a, b, li), (c, d, lj)) = (segs[i], segs[j]);
                if li == lj && (b == c || a == d) {
                    continue;
                }
                assert!(!meet(a, b, c, d), "zoom {z}: lines {li} and {lj} meet");
            }
        }
    }
}

fn world_box(w: f64, s: f64, e: f64, n: f64) -> [f64; 4] {
    let (x0, y1) = roadcore::geo::merc(w, s);
    let (x1, y0) = roadcore::geo::merc(e, n);
    [x0, y0, x1, y1]
}

#[test]
fn neighbours_share_one_arc_at_every_zoom() {
    // A border two regions share, its first stretch straight, where the east's copy has a point
    // more than the west's (a T-junction: the west's edge lies along two of the east's).
    let mut b = vec![e7(10.0, 40.0)];
    b.extend(wiggle(10.0, 40.5, 42.0, 0.2, 300, 7));
    let (w, _) = split_box(9.0, 40.0, 11.0, 42.0, &b);
    let mut be = b.clone();
    be.insert(1, e7(10.0, 40.25));
    let (_, e) = split_box(9.0, 40.0, 11.0, 42.0, &be);
    let regions = vec![region("east", &["c"], vec![e]), region("west", &["c"], vec![w])];
    let (topo, s, _) = run(&regions);
    let shared: Vec<usize> = (0..topo.arcs.len()).filter(|&k| topo.arcs[k].l != 0 && topo.arcs[k].r != 0).collect();
    assert_eq!(shared.len(), 1, "one arc between them: {:?}", topo.arcs.iter().map(|a| (a.l, a.r, a.pts.len())).collect::<Vec<_>>());
    let k = shared[0];
    let a = &topo.arcs[k];
    assert_eq!(a.pts.len(), be.len(), "the border's every point, the east's extra one too");
    assert!(topo.stats.split > 0, "the west's edge split at the east's point");
    assert!(has(&topo, a.l, "west") && has(&topo, a.r, "east") || has(&topo, a.l, "east") && has(&topo, a.r, "west"));
    assert_eq!(topo.level(a), 1, "within one folder");
    check_every_zoom(&topo, &s, world_box(9.0, 40.0, 11.0, 42.0));
    // Coarse zooms keep few of its points, the finest all.
    assert!(s.lines[k].kept(0).count() < 10 && s.lines[k].kept(2).count() < be.len() / 4);
    assert_eq!(s.lines[k].kept(MAXZ).count(), be.len());
}

#[test]
fn land_and_water_rounded_apart_are_one_region() {
    // A region's land and its water, cut from one another: the coast is the land's, but where the
    // water's copy met an edge it was rounded to the grid, a unit off the land's line on either
    // side. No line along the coast: the region's border only, the outer box.
    let coast = wiggle(10.0, 40.0, 41.0, 0.1, 40, 3);
    let mut wet = coast.clone();
    for (k, p) in wet.iter_mut().enumerate().skip(1).take(coast.len() - 2) {
        if k % 5 == 0 {
            p[0] += if k % 10 == 0 { 1 } else { -1 };
        }
    }
    // (And a point the water has on a land edge, a unit off it: a T-junction, rounded.)
    let (a, b) = (coast[1], coast[2]);
    wet.insert(2, [(a[0] + b[0]) / 2 + 1, (a[1] + b[1]) / 2]);
    let (land, _) = split_box(9.0, 40.0, 11.0, 41.0, &coast);
    let (_, water) = split_box(9.0, 40.0, 11.0, 41.0, &wet);
    let regions = vec![region("r", &[], vec![land, water])];
    let (topo, s, _) = run(&regions);
    assert_eq!(topo.arcs.len(), 1, "{:?}", topo.arcs.iter().map(|a| (a.l, a.r, a.pts.len())).collect::<Vec<_>>());
    let on_box = |p: &[i32; 2]| p[0] == e7(9.0, 0.0)[0] || p[0] == e7(11.0, 0.0)[0] || p[1] == e7(0.0, 40.0)[1] || p[1] == e7(0.0, 41.0)[1];
    assert!(topo.arcs[0].closed() && topo.arcs[0].pts.iter().all(on_box), "the box: {:?}", topo.arcs[0].pts);
    check_every_zoom(&topo, &s, world_box(9.0, 40.0, 11.0, 41.0));
}

#[test]
fn slivers_between_neighbours_are_closed() {
    // Two neighbours whose border copies part for a stretch: the west's bulges east 3 m into the
    // east's (an overlap), further north the east's falls back 3 m (a gap), as the cover check
    // leaves them under a hair. Each sliver is closed: one border, no line along a sliver.
    let b = wiggle(10.0, 40.0, 41.0, 0.05, 60, 11);
    let off = |r: std::ops::Range<usize>, d: i32| {
        let mut v = b.clone();
        for p in &mut v[r] {
            p[0] += d;
        }
        v
    };
    // (300 E7 units: about 25 m east–west at 40°N, a sliver at the finest zoom's tolerance.)
    let west_b = off(10..20, 300);
    let east_b = off(35..45, 300);
    let (w, _) = split_box(9.0, 40.0, 11.0, 41.0, &west_b);
    let (_, e) = split_box(9.0, 40.0, 11.0, 41.0, &east_b);
    let regions = vec![region("east", &["c"], vec![e]), region("west", &["c"], vec![w])];
    let (topo, s, _) = run(&regions);
    assert_eq!(topo.stats.slivers, 2, "{:?}", topo.stats);
    let shared: Vec<&topo::Arc> = topo.arcs.iter().filter(|a| a.l != 0 && a.r != 0).collect();
    assert_eq!(shared.len(), 1, "{:?}", topo.arcs.iter().map(|a| (a.l, a.r, a.pts.len())).collect::<Vec<_>>());
    assert!(topo.arcs.iter().all(|a| a.l != a.r));
    // (The border's one line, the rest the box: four sides, the border's two ends on them.)
    assert_eq!(topo.arcs.len(), 3, "{:?}", topo.arcs.iter().map(|a| (a.l, a.r, a.pts.len())).collect::<Vec<_>>());
    let _ = s;
    // Wider (3 km), they are the geometry's and stay.
    let west_b = off(10..20, 30_000);
    let (w, _) = split_box(9.0, 40.0, 11.0, 41.0, &west_b);
    let (_, e) = split_box(9.0, 40.0, 11.0, 41.0, &b);
    let (topo, _, _) = run(&[region("east", &["c"], vec![e]), region("west", &["c"], vec![w])]);
    assert_eq!(topo.stats.slivers, 0);
}

#[test]
fn a_narrow_strip_stays_whole() {
    // Three regions: a strip between two borders 250 m apart (the east one the west one moved east),
    // each wandering 20 km: simplified apart, they would cross at most zooms.
    let b1 = wiggle(10.0, 40.0, 42.0, 0.2, 400, 1);
    // (East of the west one at each point, by 250 m and up to 2 km more: so at every latitude.)
    let b2: Vec<[i32; 2]> = b1.iter().enumerate().map(|(i, p)| [p[0] + 22_000 + ((i as i32 * 7919) % 180_000), p[1]]).collect();
    let (west, _) = split_box(9.0, 40.0, 11.0, 42.0, &b1);
    let (_, east) = split_box(9.0, 40.0, 11.0, 42.0, &b2);
    // (Counter-clockwise: along the foot, up the east border, down the west one.)
    let mut strip = vec![b1[0]];
    strip.extend(b2.iter().copied());
    strip.extend(b1.iter().rev().take(b1.len() - 1).copied());
    let regions = vec![region("east", &[], vec![east]), region("strip", &[], vec![strip]), region("west", &[], vec![west])];
    let (topo, s, _) = run(&regions);
    check_every_zoom(&topo, &s, world_box(9.0, 40.0, 11.0, 42.0));
    assert!(s.zooms.iter().any(|z| z.restored > 0), "points given back to keep them apart: {:?}", s.zooms);
}

#[test]
fn an_island_and_an_enclave() {
    // The sea round an island region and a small islet of the land; the land with an enclave.
    let sea = vec![boxed(0.0, 0.0, 4.0, 4.0), rev(boxed(1.0, 1.0, 3.0, 3.0)), rev(boxed(3.5, 3.5, 3.5004, 3.5004))];
    let land = vec![boxed(1.0, 1.0, 3.0, 3.0), rev(boxed(1.9, 1.9, 2.1, 2.1)), boxed(3.5, 3.5, 3.5004, 3.5004)];
    let enclave = vec![boxed(1.9, 1.9, 2.1, 2.1)];
    // (A tiny region of one islet: kept at every zoom.)
    let tiny = vec![boxed(0.5, 0.5, 0.5002, 0.5002)];
    let mut sea = sea;
    sea.push(rev(boxed(0.5, 0.5, 0.5002, 0.5002)));
    let regions = vec![region("enclave", &["x"], enclave), region("land", &["x"], land), region("sea", &[], sea), region("tiny", &[], tiny)];
    let (topo, s, _) = run(&regions);
    let rings: Vec<&topo::Arc> = topo.arcs.iter().filter(|a| a.closed()).collect();
    assert_eq!(rings.len(), 5, "the box's outline, the island's, the islet's, the enclave's, the tiny region's");
    let enc = rings.iter().find(|a| has(&topo, a.l, "enclave") || has(&topo, a.r, "enclave")).unwrap();
    assert!(has(&topo, enc.l, "land") || has(&topo, enc.r, "land"));
    assert_eq!(topo.level(enc), 1);
    let isl = rings.iter().find(|a| (has(&topo, a.l, "sea") || has(&topo, a.r, "sea")) && a.pts.len() == 5 && a.pts[0] == e7(1.0, 1.0)).unwrap();
    assert_eq!(topo.level(isl), 0, "the land's coast with the sea: different top folders");
    check_every_zoom(&topo, &s, world_box(-0.5, -0.5, 4.5, 4.5));
    let at = |p: [i32; 2]| topo.arcs.iter().position(|a| a.closed() && a.pts[0] == p).unwrap();
    // The islet (40 m) is left out at coarse zooms, both sides at once; the tiny region stays.
    let islet = at(e7(3.5, 3.5));
    assert_eq!(s.lines[islet].kept(3).count(), 0);
    assert_eq!(s.lines[islet].kept(MAXZ).count(), 5);
    let t = at(e7(0.5, 0.5));
    assert!(s.lines[t].kept(0).count() >= 4, "a small island region at every zoom");
}

#[test]
fn across_the_antimeridian() {
    // "fiji" across ±180°, two rings; "tonga" east of it on the west side only for part of the
    // latitudes: across the meridian there it's fiji on one side, tonga on the other.
    let fiji = vec![boxed(178.0, -20.0, 180.0, -15.0), boxed(-180.0, -20.0, -178.0, -15.0)];
    let tonga = vec![boxed(-180.0, -25.0, -176.0, -20.0)];
    let wsea = vec![boxed(176.0, -25.0, 180.0, -20.0)];
    let regions = vec![region("fiji", &[], fiji), region("tonga", &[], tonga), region("wsea", &[], wsea)];
    let (topo, _, tiles) = run(&regions);
    let x = |a: &topo::Arc| a.pts.iter().all(|p| p[0].abs() == HALF);
    // Fiji's cut: on both meridians, not drawn; tonga against wsea across it: drawn on both.
    let cut: Vec<&topo::Arc> = topo.arcs.iter().filter(|a| x(a) && !a.drawn).collect();
    let drawn: Vec<&topo::Arc> = topo.arcs.iter().filter(|a| x(a) && a.drawn).collect();
    assert_eq!(cut.len(), 2, "fiji's cut on each meridian: {:?}", topo.arcs.iter().filter(|a| x(a)).map(|a| (&a.pts, a.drawn)).collect::<Vec<_>>());
    assert!(cut.iter().all(|a| a.pts.iter().all(|p| p[1] >= -200_000_000)));
    assert_eq!(drawn.len(), 2);
    for a in drawn {
        let (l, r) = (topo.set_text(a.ld), topo.set_text(a.rd));
        assert!((l == "wsea" && r == "tonga") || (l == "tonga" && r == "wsea"), "{l} | {r}");
    }
    // No line in a tile along x = 0 or the tile's east edge at the world's edge, for fiji.
    for z in [3u8, 6, 9] {
        let n = 1u32 << z;
        let (_, ty) = (0, ((roadcore::geo::merc(0.0, -17.5).1) * n as f64) as u32);
        for tx in [0, n - 1] {
            let Some(b) = tiles.get(&(z, tx, ty)) else { continue };
            let t = names::mvt::Tile::decode(b).unwrap();
            for f in &t.layers[0].features {
                let l = f.tags.chunks(2).map(|kv| t.layers[0].values[kv[1] as usize].as_str().unwrap_or("").to_string()).collect::<Vec<_>>();
                assert!(!(l[0] == "fiji" && l[1] == "fiji"), "a cut drawn");
            }
        }
    }
}

#[test]
fn antarctica_to_the_pole() {
    // Antarctica from 60° S to the pole, all the way round, with the sea north of it.
    let ant = vec![vec![e7(-180.0, -90.0), e7(180.0, -90.0), e7(180.0, -60.0), e7(0.0, -60.0), e7(-180.0, -60.0)]];
    let sea = vec![vec![e7(-180.0, -60.0), e7(0.0, -60.0), e7(180.0, -60.0), e7(180.0, -40.0), e7(-180.0, -40.0)]];
    let regions = vec![region("antarctica", &[], ant), region("sea", &[], sea)];
    let (topo, s, tiles) = run(&regions);
    for a in topo.arcs.iter().filter(|a| a.drawn) {
        let along = |x: i32| a.pts.iter().all(|p| p[0] == x);
        assert!(!along(HALF) && !along(-HALF) && !a.pts.iter().all(|p| p[1] == -POLE), "a cut drawn: {:?}", a.pts);
    }
    assert!(topo.arcs.iter().any(|a| a.drawn && a.pts.iter().all(|p| p[1] == -600_000_000)));
    check_every_zoom(&topo, &s, world_box(-179.0, -84.0, 179.0, -39.0));
    // No line along the tiles' southern edge (y = 1: 85.05° S): the cut at the pole isn't drawn.
    for (&(z, _, y), b) in &tiles {
        if y != (1 << z) - 1 {
            continue;
        }
        let t = names::mvt::Tile::decode(b).unwrap();
        let e = t.layers[0].extent as i32;
        for f in &t.layers[0].features {
            let pts = decode(&f.geometry);
            for line in pts {
                for w in line.windows(2) {
                    assert!(!(w[0].1 >= e && w[1].1 >= e), "zoom {z}: a line along the tiles' edge");
                }
            }
        }
    }
}

/// A feature's lines in tile units.
fn decode(g: &[u32]) -> Vec<Vec<(i32, i32)>> {
    let un = |v: u32| ((v >> 1) as i32) ^ -((v & 1) as i32);
    let (mut out, mut i, mut cur) = (Vec::<Vec<(i32, i32)>>::new(), 0, (0, 0));
    while i < g.len() {
        let (cmd, n) = (g[i] & 7, g[i] >> 3);
        i += 1;
        for _ in 0..n {
            if cmd == 7 {
                break;
            }
            cur = (cur.0 + un(g[i]), cur.1 + un(g[i + 1]));
            i += 2;
            if cmd == 1 {
                out.push(vec![cur]);
            } else {
                out.last_mut().unwrap().push(cur);
            }
        }
    }
    out
}

#[test]
fn a_border_across_a_tile_edge_is_whole() {
    // A border along 10° E crossing z4's tile row edge at 40.98° N (the tiles' rows: Mercator).
    let b = wiggle(10.0, 38.0, 44.0, 0.05, 200, 3);
    let (w, e) = split_box(9.0, 38.0, 11.0, 44.0, &b);
    let (topo, _, tiles) = run(&[region("e", &[], vec![e]), region("w", &[], vec![w])]);
    let z = 4u8;
    let n = 1u32 << z;
    let (wx, _) = roadcore::geo::merc(10.0, 41.0);
    let tx = (wx * n as f64) as u32;
    // The rows either side of the edge nearest 41° N.
    let ty = (roadcore::geo::merc(10.0, 41.0).1 * n as f64).round() as u32;
    let (upper, lower) = (tiles.get(&(z, tx, ty - 1)).unwrap(), tiles.get(&(z, tx, ty)).unwrap());
    let ext = |b: &[u8]| {
        let t = names::mvt::Tile::decode(b).unwrap();
        let e = t.layers[0].extent as i32;
        let shared: Vec<Vec<(i32, i32)>> = t.layers[0]
            .features
            .iter()
            .filter(|f| f.tags.chunks(2).all(|kv| t.layers[0].values[kv[1] as usize].as_str().is_none_or(|s| !s.is_empty())))
            .flat_map(|f| decode(&f.geometry))
            .collect();
        (e, shared)
    };
    let ((e1, a), (e2, b)) = (ext(upper), ext(lower));
    // The upper tile's line runs past its bottom edge into the buffer, the lower's past its top.
    assert!(a.iter().flatten().any(|p| p.1 > e1), "into the upper tile's buffer");
    assert!(b.iter().flatten().any(|p| p.1 < 0), "into the lower tile's buffer");
    assert!(a.iter().flatten().all(|p| p.1 <= e1 + e1 / 64 + 1) && b.iter().flatten().all(|p| p.1 >= -e2 / 64 - 1));
    let _ = topo;
}

#[test]
fn the_regions_at_a_point() {
    let b = wiggle(10.0, 40.0, 42.0, 0.2, 300, 7);
    let (w, e) = split_box(9.0, 40.0, 11.0, 42.0, &b);
    let mut e = vec![e];
    // An island of the east region in the west's.
    e.push(boxed(9.2, 41.0, 9.21, 41.01));
    let w = vec![w, rev(boxed(9.2, 41.0, 9.21, 41.01))];
    let (_, _, tiles) = run(&[region("east", &[], e.clone()), region("west", &[], w)]);
    let mut get = |z: u8, x: u32, y: u32| -> anyhow::Result<Option<Vec<u8>>> { Ok(tiles.get(&(z, x, y)).cloned()) };
    let at = |lon: f64, lat: f64, get: &mut dyn FnMut(u8, u32, u32) -> anyhow::Result<Option<Vec<u8>>>| hit::regions_at(get, lon, lat).unwrap();
    assert_eq!(at(9.1, 41.5, &mut get), ["west"]);
    assert_eq!(at(10.9, 41.5, &mut get), ["east"]);
    assert_eq!(at(9.205, 41.005, &mut get), ["east"], "on the island");
    assert_eq!(at(9.22, 41.005, &mut get), ["west"], "just off it");
    assert!(at(12.0, 41.0, &mut get).is_empty(), "no region");
    assert!(at(8.95, 41.0, &mut get).is_empty(), "just past the west's coast");
    assert!(at(0.0, 88.0, &mut get).is_empty(), "past the tiles");
    // Either side of the wandering border, a few hundred metres off it.
    for (i, p) in b.iter().enumerate().skip(1).step_by(37) {
        let (lon, lat) = (p[0] as f64 * 1e-7, p[1] as f64 * 1e-7);
        let _ = i;
        let w = at(lon - 0.004, lat, &mut get);
        let e = at(lon + 0.004, lat, &mut get);
        assert!(w != e, "at {lon}, {lat}: {w:?} {e:?}");
    }
}

#[test]
fn a_node_where_three_meet() {
    // Three regions meeting at a point: the regions round it, from its tiles, each where it is.
    let c = e7(5.0, 5.0);
    let a = vec![e7(0.0, 0.0), e7(10.0, 0.0), c];
    let b = vec![e7(10.0, 0.0), e7(10.0, 10.0), c];
    let d = vec![e7(10.0, 10.0), e7(0.0, 10.0), e7(0.0, 0.0), c];
    let (topo, _, tiles) = run(&[region("a", &[], vec![a]), region("b", &[], vec![b]), region("d", &[], vec![d])]);
    assert_eq!(topo.arcs.iter().filter(|x| x.l != 0 && x.r != 0).count(), 3);
    let mut get = |z: u8, x: u32, y: u32| -> anyhow::Result<Option<Vec<u8>>> { Ok(tiles.get(&(z, x, y)).cloned()) };
    // (North-west of the node, its nearest point is the node itself.)
    for (lon, lat, want) in [(5.0, 4.999, "a"), (5.001, 5.0, "b"), (4.999, 5.0, "d"), (5.0, 5.001, "d"), (4.999, 5.001, "d"), (5.0, 4.99, "a")] {
        assert_eq!(hit::regions_at(&mut get, lon, lat).unwrap(), [want], "{lon}, {lat}");
    }
}

#[test]
fn overlapping_regions_keep_their_own_borders() {
    // Where regions overlap (no cover rule), each keeps its border, crossing the other's, and is
    // drawn whole; the regions at a point there are those the nearest border has on that side,
    // which may leave one out (the topology has no faces of the overlap: docs/steps.md §15).
    let a = vec![boxed(0.0, 0.0, 2.0, 2.0)];
    let b = vec![boxed(1.0, 1.0, 3.0, 3.0)];
    let (topo, _, tiles) = run(&[region("a", &[], a), region("b", &[], b)]);
    assert_eq!(topo.arcs.len(), 2);
    assert!(topo.arcs.iter().all(|a| a.drawn && a.closed()));
    let mut get = |z: u8, x: u32, y: u32| -> anyhow::Result<Option<Vec<u8>>> { Ok(tiles.get(&(z, x, y)).cloned()) };
    let both = hit::regions_at(&mut get, 1.5, 1.4).unwrap();
    assert!(both == ["a"] || both == ["b"], "{both:?}");
    assert_eq!(hit::regions_at(&mut get, 0.2, 0.2).unwrap(), ["a"]);
}

#[test]
fn pack_layout() {
    assert_eq!(pack_of(2, 3, 1), ("root", (0, 0, 0)));
    assert_eq!(pack_of(5, 20, 9), ("lo", (3, 5, 2)));
    assert_eq!(pack_of(12, 2049, 1400), ("hi", (6, 32, 21)));
    assert!(tolerance(12) < tolerance(0));
}

#[test]
fn a_hair_of_a_region_on_its_own_edge() {
    // A region's ring with a hair: out along an edge and back 2 units off it, rounded so (as on
    // the Nunavut coast). Split onto the edge, the hair cancels: one closed ring, no gap.
    let a = [-940293830, 611769680];
    let a2 = [-940293828, 611769680];
    let c = [-940252851, 611767923];
    let ring = vec![[-940400000, 611700000], [-940200000, 611700000], [-940200000, 611760000], a, c, a2, a, [-940400000, 611800000]];
    let t = build(&[region("n", &[], vec![ring])]);
    assert_eq!(t.arcs.len(), 1, "{:?}", t.arcs);
    assert!(t.arcs[0].closed() && !t.arcs[0].pts.contains(&c), "{:?}", t.arcs[0].pts);
}
