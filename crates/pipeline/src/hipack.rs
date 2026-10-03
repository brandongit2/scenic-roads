//! Per-area packs from base packs (docs/plan.md §6): pack(T) for a z6 tile T (road and rail tiles
//! z9–14, and hidata: the ways drawn here, query parts, climbs) and lo packs for a z3 tile (road and
//! rail tiles z4–8). Each reads the base packs of every unit whose geometry reaches the area (plus a
//! 100 km halo for climbs), never anything per region.

use crate::basepack::BasePack;
use crate::climbs;
use crate::legacy::Unit;
use crate::tiling::{self, Encoded, WayIn};
use anyhow::Result;
use rayon::prelude::*;
use roadcore::scenic::{ch, Sample};
use roadcore::tile::TileLine;
use roadcore::{class, dist_m, flag, WayRec, E7};

/// [west, south, east, north] in E7 degrees.
pub type Bounds = [i32; 4];

/// A tile's bounds.
pub fn tile_bounds(z: u8, x: u32, y: u32) -> Bounds {
    let n = (1u64 << z) as f64;
    let lon = |x: f64| x / n * 360.0 - 180.0;
    let lat = |y: f64| (std::f64::consts::PI * (1.0 - 2.0 * y / n)).sinh().atan().to_degrees();
    let e7 = |d: f64| (d / E7).round() as i32;
    [e7(lon(x as f64)), e7(lat(y as f64 + 1.0)), e7(lon(x as f64 + 1.0)), e7(lat(y as f64))]
}

/// Bounds grown by `km` on every side.
pub fn grow(b: Bounds, km: f64) -> Bounds {
    let dlat = km / 111.32;
    let lat_max = (b[1].unsigned_abs().max(b[3].unsigned_abs()) as f64 * E7 + dlat).min(89.0);
    let dlon = (dlat / lat_max.to_radians().cos()).min(180.0);
    let e7 = |d: f64| (d / E7).round() as i64;
    let c = |v: i64| v.clamp(-1_800_000_000, 1_800_000_000) as i32;
    [c(b[0] as i64 - e7(dlon)), c(b[1] as i64 - e7(dlat)).max(-850_000_000), c(b[2] as i64 + e7(dlon)), c(b[3] as i64 + e7(dlat)).min(850_000_000)]
}

pub fn meets(a: Bounds, b: Bounds) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

fn inside(p: [i32; 2], b: Bounds) -> bool {
    p[0] >= b[0] && p[0] < b[2] && p[1] >= b[1] && p[1] < b[3]
}

/// A way of one base pack: (pack index, way index, bbox).
#[derive(Clone, Copy)]
pub struct Staged {
    pub pack: usize,
    pub way: u32,
    pub bbox: Bounds,
}

/// Ways of `packs` whose bbox meets `area`.
pub fn ways_in(packs: &[&BasePack], area: Bounds) -> Result<Vec<Staged>> {
    let per: Vec<Vec<Staged>> = packs
        .par_iter()
        .enumerate()
        .map(|(pi, bp)| -> Result<Vec<Staged>> {
            if !meets(bp.extent, area) {
                return Ok(Vec::new());
            }
            Ok(bp
                .bboxes()?
                .iter()
                .enumerate()
                .filter(|(_, bb)| meets(**bb, area))
                .map(|(wi, bb)| Staged { pack: pi, way: wi as u32, bbox: *bb })
                .collect())
        })
        .collect::<Result<_>>()?;
    Ok(per.into_iter().flatten().collect())
}

/// The tiler's view of staged ways.
pub fn way_inputs<'a>(packs: &[&'a BasePack], staged: &[Staged]) -> Result<Vec<WayIn<'a>>> {
    staged
        .iter()
        .map(|s| {
            let bp = packs[s.pack];
            let w = &bp.ways()?[s.way as usize];
            let r = bp.range(w);
            Ok(WayIn {
                rec: w,
                id: w.id as u32,
                verts: &bp.verts()?[r.clone()],
                elev_dm: bp.elev()?.slice(r.clone()),
                grade: &bp.grade()?[r.clone()],
                drape: bp.drape().map(|d| &d[r.clone()]),
                sc: bp.scenic().map(|a| &a[r.clone()]),
                surface: bp.string(w.surface),
                road_m: bp.road_vals()?[s.way as usize].len.round() as u32,
            })
        })
        .collect()
}

/// Road and rail tiles for zooms `zs` of the tiles under (zr, xr, yr), from `ways`.
pub fn tiles(ways: &[WayIn], zr: u8, xr: u32, yr: u32, zs: std::ops::RangeInclusive<u8>, progress: bool) -> (Vec<Encoded>, Vec<Encoded>) {
    let (mut roads, mut rails) = (Vec::new(), Vec::new());
    for z in zs {
        let sh = z - zr;
        let keep = move |x: u32, y: u32| x >> sh == xr && y >> sh == yr;
        let pieces = tiling::cut(z, ways, &keep, progress);
        let (rp, dp): (Vec<(u64, TileLine)>, Vec<(u64, TileLine)>) = pieces.into_iter().partition(|p| class::is_rail(p.1.style & 0x0f));
        roads.extend(tiling::encode_zoom(z, 14, &dp, None));
        rails.extend(tiling::encode_zoom(z, 14, &rp, None));
    }
    (roads, rails)
}

pub use roadcore::packs::{here_extra as extra, point_key, Climb, End, Here, PSample, Part, RailInfo};

/// hidata sections of a z6 tile.
pub struct HiData {
    pub here: Vec<Here>,
    pub ends: Vec<End>,
    pub parts: Vec<Part>,
    pub psamples: Vec<PSample>,
    pub pch: Vec<[u8; ch::N]>,
    pub climbs: Vec<Climb>,
    pub climbgeom: Vec<[i32; 2]>,
    /// The rail ways' lines, and their strings (newline-separated; 0 is "").
    pub railinfo: Vec<RailInfo>,
    pub railstr: Vec<u8>,
}

fn way_len(v: &[[i32; 2]]) -> f64 {
    v.windows(2).map(|p| dist_m(p[0][0] as f64 * E7, p[0][1] as f64 * E7, p[1][0] as f64 * E7, p[1][1] as f64 * E7)).sum()
}

/// The hidata of z6 tile `t`: `in_t` are the ways meeting T, `halo` the ways meeting T + 100 km.
pub fn hidata(t: Unit, packs: &[&BasePack], in_t: &[Staged], halo: &[Staged]) -> Result<HiData> {
    let tb = tile_bounds(t.z, t.x, t.y);
    // here: sorted by OSM id.
    let mut here: Vec<Here> = in_t
        .iter()
        .map(|s| -> Result<Here> {
            let bp = packs[s.pack];
            let w = &bp.ways()?[s.way as usize];
            let mut extra = 0u8;
            if w.name == 0 && w.ref_ == 0 {
                extra |= extra::UNNAMED;
            }
            if class::is_rail(w.class) {
                extra |= extra::RAIL;
            }
            Ok(Here { id: w.id as u64, owner: bp.unit.key(), index: s.way, class: w.class, flags: w.flags, extra, _pad: 0, bbox: s.bbox })
        })
        .collect::<Result<_>>()?;
    here.sort_unstable_by_key(|h| (h.id, h.owner, h.index));
    here.dedup_by_key(|h| h.id);
    let pos_of = |pack: usize, way: u32| -> Option<u32> {
        let id = packs[pack].ways().ok()?[way as usize].id as u64;
        here.binary_search_by_key(&id, |h| h.id).ok().map(|i| i as u32)
    };
    // ends
    let mut ends: Vec<End> = Vec::with_capacity(here.len() * 2);
    for s in in_t {
        let bp = packs[s.pack];
        let w = &bp.ways()?[s.way as usize];
        let v = &bp.verts()?[bp.range(w)];
        if let Some(hi) = pos_of(s.pack, s.way) {
            for p in [v[0], v[v.len() - 1]] {
                ends.push(End { point: point_key(p), here: hi, _pad: 0 });
            }
        }
    }
    ends.sort_unstable_by_key(|e| (e.point, e.here));
    ends.dedup_by_key(|e| (e.point, e.here));
    // Query parts: every sample inside T, along its road.
    struct Q {
        road: u64,
        road_len: f32,
        off: f32,
        here: u32,
        s: Sample,
        c: [u8; ch::N],
    }
    let mut qs: Vec<Q> = Vec::new();
    for s in in_t {
        let bp = packs[s.pack];
        let w = &bp.ways()?[s.way as usize];
        if w.class == class::FERRY {
            continue;
        }
        let Some(hi) = pos_of(s.pack, s.way) else { continue };
        let samples = bp.samples()?;
        let chs = bp.samplech()?;
        let a = samples.partition_point(|x| x.way < s.way);
        let b = samples.partition_point(|x| x.way <= s.way);
        if a == b {
            continue;
        }
        let rv = bp.road_vals()?[s.way as usize];
        let wl = way_len(&bp.verts()?[bp.range(w)]) as f32;
        for k in a..b {
            let smp = samples[k];
            if !inside([smp.lon, smp.lat], tb) {
                continue;
            }
            let along = if rv.dir == 0 { smp.dist } else { wl - smp.dist };
            qs.push(Q { road: rv.road, road_len: rv.len, off: rv.offset + along, here: hi, s: smp, c: chs[k] });
        }
    }
    qs.par_sort_unstable_by(|a, b| a.road.cmp(&b.road).then(a.off.total_cmp(&b.off)).then(a.here.cmp(&b.here)));
    let mut parts: Vec<Part> = Vec::new();
    let mut psamples: Vec<PSample> = Vec::with_capacity(qs.len());
    let mut pch: Vec<[u8; ch::N]> = Vec::with_capacity(qs.len());
    for (i, q) in qs.iter().enumerate() {
        let new_part = i == 0 || qs[i - 1].road != q.road || q.off - qs[i - 1].off > 300.0;
        if new_part {
            let h = &here[q.here as usize];
            let mut fl = 0u8;
            if h.flags & flag::UNPAVED != 0 {
                fl |= 1;
            }
            if h.flags & flag::TOLL != 0 {
                fl |= 2;
            }
            if h.extra & extra::UNNAMED != 0 {
                fl |= 4;
            }
            parts.push(Part { road: q.road, offset: q.off, first: psamples.len() as u32, count: 0, road_len: q.road_len, class: h.class, flags: fl, _pad: [0; 6] });
        }
        let p = parts.last_mut().unwrap();
        p.count += 1;
        psamples.push(PSample { way: q.here, offset: q.off, lon: q.s.lon, lat: q.s.lat, eye: q.s.eye, flags: q.s.flags, _pad: [0; 3] });
        pch.push(q.c);
    }
    // Climbs along roads through the halo, kept when they start in T.
    let (climbs, climbgeom) = climbs_in(t, packs, halo)?;
    // The rail ways' lines (name, route, colour, relation, services), by their place in `here`.
    let mut strings: Vec<String> = vec![String::new()];
    let mut index: std::collections::HashMap<String, u32> = std::collections::HashMap::from([(String::new(), 0)]);
    let mut intern = |s: &str| -> u32 {
        if let Some(&i) = index.get(s) {
            return i;
        }
        strings.push(s.replace('\n', " "));
        index.insert(s.to_string(), (strings.len() - 1) as u32);
        (strings.len() - 1) as u32
    };
    let mut railinfo: Vec<RailInfo> = Vec::new();
    for s in in_t {
        let bp = packs[s.pack];
        let w = &bp.ways()?[s.way as usize];
        if !class::is_rail(w.class) {
            continue;
        }
        let Ok(hi) = here.binary_search_by_key(&(w.id as u64), |h| h.id) else { continue };
        // (A way in several packs' halos: the one `here` keeps.)
        if here[hi].owner != bp.unit.key() || here[hi].index != s.way {
            continue;
        }
        railinfo.push(RailInfo {
            here: hi as u32,
            colour: w.colour,
            rel: bp.rail_rel(s.way).unwrap_or(0),
            name: intern(bp.string(w.name)),
            route: intern(bp.string(w.route)),
            rail: w.rail,
            class: w.class,
            _pad: [0; 6],
        });
    }
    railinfo.sort_unstable_by_key(|r| r.here);
    railinfo.dedup_by_key(|r| r.here);
    let railstr = strings.join("\n").into_bytes();
    Ok(HiData { here, ends, parts, psamples, pch, climbs, climbgeom, railinfo, railstr })
}

/// Climbs starting in T, found along the roads of the halo's ways (in order by road offset).
fn climbs_in(t: Unit, packs: &[&BasePack], halo: &[Staged]) -> Result<(Vec<Climb>, Vec<[i32; 2]>)> {
    let tb = tile_bounds(t.z, t.x, t.y);
    // Staging arrays: the halo's road ways, copied into one set of arrays.
    let mut ways: Vec<WayRec> = Vec::new();
    let mut verts: Vec<[i32; 2]> = Vec::new();
    let mut elev: Vec<f32> = Vec::new();
    let mut ids: Vec<u64> = Vec::new();
    // Per staging way: the road's length and the climb flags (toll, unnamed).
    let mut info: Vec<(f32, u8)> = Vec::new();
    let mut keyed: Vec<(u64, f32, u32, bool, f32)> = Vec::new(); // road, offset, staging idx, reversed, length
    for s in halo {
        let bp = packs[s.pack];
        let w = bp.ways()?[s.way as usize];
        if w.class == class::FERRY || class::is_rail(w.class) || w.vcount < 2 {
            continue;
        }
        let r = bp.range(&w);
        let v = &bp.verts()?[r.clone()];
        let mut nw = w;
        nw.vstart = verts.len() as u64;
        verts.extend_from_slice(v);
        let e = bp.elev()?.slice(r);
        elev.extend((0..e.len()).map(|i| e.m(i)));
        let rv = bp.road_vals()?[s.way as usize];
        keyed.push((rv.road, rv.offset, ways.len() as u32, rv.dir == 1, way_len(v) as f32));
        ids.push(w.id as u64);
        info.push((rv.len, (w.flags & flag::TOLL != 0) as u8 | ((w.name == 0 && w.ref_ == 0) as u8) << 1));
        ways.push(nw);
    }
    keyed.par_sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
    // Strokes: runs of a road's ways that follow each other without a gap.
    let mut strokes: Vec<Vec<climbs::OWay>> = Vec::new();
    for (i, k) in keyed.iter().enumerate() {
        let contiguous = i > 0 && keyed[i - 1].0 == k.0 && (k.1 - (keyed[i - 1].1 + keyed[i - 1].4)).abs() < 1.0;
        if !contiguous {
            strokes.push(Vec::new());
        }
        strokes.last_mut().unwrap().push((k.2, k.3));
    }
    let cl = climbs::find_on(&ways, &verts, &elev, strokes);
    let mut out = Vec::new();
    let mut geom = Vec::new();
    for c in &cl.recs {
        let g = &cl.geom[c.geom_start as usize..(c.geom_start + c.geom_count) as usize];
        if g.is_empty() || !inside(g[0], tb) {
            continue;
        }
        out.push(Climb {
            way: ids[c.way as usize],
            label: ids[c.label_way as usize],
            gain_m: c.gain_m,
            length_m: c.length_m,
            start_elev: c.start_elev,
            top_elev: c.top_elev,
            max_grade: c.max_grade,
            road_len: info[c.label_way as usize].0,
            mid: c.mid,
            geom_start: geom.len() as u32,
            geom_count: g.len() as u32,
            class: c.class,
            unpaved: c.unpaved,
            flags: info[c.label_way as usize].1,
            _pad: [0; 5],
        });
        geom.extend_from_slice(g);
    }
    Ok((out, geom))
}

/// Bytes of a slice of records.
pub fn b<T: bytemuck::Pod>(v: &[T]) -> &[u8] {
    bytemuck::cast_slice(v)
}
