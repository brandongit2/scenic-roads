//! The depth steps on fixtures: a z6 tile of sea and land with a lake, a coarse sea source over it
//! all, a finer one over a patch of the sea, and a lake survey.

use super::build::{build_hi, build_lo, hi_logical, lo_logical, square};
use super::fall::{self, Grid, Kind};
use super::grids::{tests::grid_file, window_box, Grid as SGrid, Source};
use super::*;
use crate::out::Out;
use crate::terrain_water::{Kind as WKind, Poly, WaterSource};
use std::path::Path;

/// The fixture's z6 tile.
const T: (u32, u32) = (19, 23);

/// A rectangle of the z6 tile `T` (u, v from 0 to 1 across it) in tile z/x/y's pixels; None when
/// it misses the tile.
fn rect(z: u8, x: u32, y: u32, u0: f64, v0: f64, u1: f64, v1: f64) -> Option<[f64; 4]> {
    let n = f64::from(1u32 << (z - 6));
    let to = |u: f64, base: u32, t: u32| ((f64::from(base) + u) * n - f64::from(t)) * 256.0;
    let (x0, x1, y0, y1) = (to(u0, T.0, x), to(u1, T.0, x), to(v0, T.1, y), to(v1, T.1, y));
    (x1 > 0.0 && x0 < 256.0 && y1 > 0.0 && y0 < 256.0).then_some([
        x0.max(-4.0),
        y0.max(-4.0),
        x1.min(260.0),
        y1.min(260.0),
    ])
}

/// The water: the sea west of the middle of `T`, a lake (OSM id 77) in its north-east.
struct Fixture;
impl WaterSource for Fixture {
    fn polys(&self, z: u8, x: u32, y: u32) -> anyhow::Result<Vec<Poly>> {
        if !(6..=14).contains(&z) {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        if let Some(r) = rect(z, x, y, 0.0, 0.0, 0.5, 1.0) {
            out.push(square(WKind::Sea, 0, r[0], r[1], r[2], r[3]));
        }
        if let Some(r) = rect(z, x, y, 0.7, 0.2, 0.8, 0.3) {
            out.push(square(WKind::Lake, 77, r[0], r[1], r[2], r[3]));
        }
        Ok(out)
    }
    fn pin(&self) -> String {
        "fixture".into()
    }
}

/// The global pixel (column, row) of (u, v) in `T` at zoom z.
fn at(z: u8, u: f64, v: f64) -> (i64, i64) {
    let n = f64::from(1u32 << (z - 6)) * 256.0;
    (((f64::from(T.0) + u) * n) as i64, ((f64::from(T.1) + v) * n) as i64)
}

fn sources(dir: &Path) -> Vec<Source> {
    let mk = |code: u8, lake: bool, zoom: u8, u0: f64, v0: f64, u1: f64, v1: f64, f: &dyn Fn(i64, i64) -> f32| {
        let (c0, r0) = at(zoom, u0, v0);
        let (c1, r1) = at(zoom, u1, v1);
        let p = dir.join(format!("{code}.tif"));
        grid_file(&p, zoom, c0, r0, (c1 - c0) as u32, (r1 - r0) as u32, f);
        Source {
            id: format!("s{code}"),
            code,
            lake,
            band_m: 2000.0,
            intertidal: false,
            datum_exact: true,
            rivers: false,
            estimated: false,
            label: format!("S{code}"),
            datum: "MSL".into(),
            notice: None,
            grids: vec![SGrid {
                bbox: window_box(zoom, c0, r0, (c1 - c0) as usize, (r1 - r0) as usize),
                zoom,
                file: p,
                zoom_file: None,
                low_file: None,
                packed: None,
            }],
            pin: format!("s{code}"),
        }
    };
    vec![
        // The whole tile, deep (z8).
        mk(1, false, 8, 0.0, 0.0, 1.0, 1.0, &|_, _| -50.0),
        // A patch of the sea, shallower (z10), inside one block of z10's.
        mk(2, false, 10, 0.1, 0.3, 0.25, 0.45, &|_, _| -10.0),
        // The lake's survey (z11), deeper to the east.
        mk(3, true, 11, 0.69, 0.19, 0.81, 0.31, &|c, _| -2.0 - (c % 100) as f32 * 0.01),
    ]
}

fn tile(out: &Out, layer: &str, scope: &str, root: (u8, u32, u32), z: u8, x: u32, y: u32) -> Option<Vec<u8>> {
    let c = out.get(&format!("layers/{layer}/{scope}/{}-{}-{}", root.0, root.1, root.2))?;
    let b = std::fs::read(out.path(c)).unwrap();
    let idx = store::pack::PackIndex::parse_bytes(&b).unwrap();
    idx.get(&b, z, x, y).unwrap().map(|(_, b)| b)
}

fn depth_at(png: &[u8], px: usize, py: usize) -> f32 {
    let Grid::F(v) = fall::decode(Kind::Terrarium, png).unwrap() else { panic!() };
    v[py * 256 + px]
}

fn class_at(png: &[u8], px: usize, py: usize) -> u8 {
    let Grid::C(v) = fall::decode(Kind::Class, png).unwrap() else { panic!() };
    v[py * 256 + px]
}

/// The tile of zoom z holding (u, v) of `T`, and the pixel in it.
fn where_(z: u8, u: f64, v: f64) -> (u32, u32, usize, usize) {
    let (c, r) = at(z, u, v);
    ((c / 256) as u32, (r / 256) as u32, (c % 256) as usize, (r % 256) as usize)
}

#[test]
fn the_depth_of_a_tile_follows_its_sources_detail() {
    let d = tempfile::tempdir().unwrap();
    let srcs = sources(d.path());
    let mut out = Out::unbound(&d.path().join("root"), &d.path().join("scratch")).unwrap();
    let q = (T.0 >> 3, T.1 >> 3);
    let n_lo = build_lo(&mut out, &srcs, &Fixture, None, q).unwrap();
    assert!(
        n_lo > 0
            && out.get(&lo_logical(LAYER, q.0, q.1)).is_some()
            && out.get(&lo_logical(CLASS_LAYER, q.0, q.1)).is_some()
    );
    let n_hi = build_hi(&mut out, &srcs, &Fixture, None, T).unwrap();
    assert!(n_hi > 0 && out.get(&hi_logical(LAYER, T.0, T.1)).is_some());
    let lo = (3, q.0, q.1);
    let hi = (6, T.0, T.1);

    // z8: the open sea deep (the coarse source), the patch a mix at most, the land no data.
    let (x, y, px, py) = where_(8, 0.45, 0.5);
    let t8 = tile(&out, LAYER, "lo", lo, 8, x, y).expect("a z8 sea tile");
    assert!((depth_at(&t8, px, py) + 50.0).abs() < 0.6, "{}", depth_at(&t8, px, py));
    let c8 = tile(&out, CLASS_LAYER, "lo", lo, 8, x, y).unwrap();
    assert_eq!(class_at(&c8, px, py), C_MEASURED | (1 << 4));
    let (x, y, px, py) = where_(8, 0.9, 0.9);
    let land = tile(&out, LAYER, "lo", lo, 8, x, y);
    assert!(land.is_none_or(|t| depth_at(&t, px, py) >= NODATA_MIN), "the land has no depth");

    // z10: the patch's own detail stored, at its value; the deep sea beyond the strip isn't
    // (the coarse source has no detail of z10 there).
    let (x, y, px, py) = where_(10, 0.2, 0.4);
    let t10 = tile(&out, LAYER, "hi", hi, 10, x, y).expect("the patch's z10 tile");
    assert!((depth_at(&t10, px, py) + 10.0).abs() < 0.11, "{}", depth_at(&t10, px, py));
    let c10 = tile(&out, CLASS_LAYER, "hi", hi, 10, x, y).unwrap();
    assert_eq!(class_at(&c10, px, py) >> 4, 2);
    let (x, y, _, _) = where_(10, 0.45, 0.05);
    assert!(tile(&out, LAYER, "hi", hi, 10, x, y).is_none(), "no z10 detail in the open sea");

    // z11: the lake's survey, below its surface; none of the sea's sources in it.
    let (x, y, px, py) = where_(11, 0.75, 0.25);
    let t11 = tile(&out, LAYER, "hi", hi, 11, x, y).expect("the lake's z11 tile");
    let v = depth_at(&t11, px, py);
    assert!((-3.1..=-1.9).contains(&v), "{v}");
    assert_eq!(class_at(&tile(&out, CLASS_LAYER, "hi", hi, 11, x, y).unwrap(), px, py) >> 4, 3);
    // Nothing finer than its sources: no z12 and deeper.
    for z in 12..=16u8 {
        let (x, y, _, _) = where_(z, 0.75, 0.25);
        assert!(tile(&out, LAYER, "hi", hi, z, x, y).is_none(), "z{z}");
    }

    // A tile the layer lacks, made from its stored ancestor, is what the composition scaled up
    // there: the open sea at z9 is the z8 tile's quadrant.
    let get = |z: u8, x: u32, y: u32| -> anyhow::Result<fall::Stored> {
        Ok(tile(&out, LAYER, "lo", lo, z, x, y)
            .map(|b| std::sync::Arc::new(fall::decode(Kind::Terrarium, &b).unwrap())))
    };
    let (x, y, px, py) = where_(9, 0.45, 0.6);
    let (g, stored) = fall::served(&get, Kind::Terrarium, 9, x, y).unwrap().unwrap();
    assert!(!stored);
    let Grid::F(v) = g else { panic!() };
    assert!((v[py * 256 + px] + 50.0).abs() < 0.6, "{}", v[py * 256 + px]);

    // The same again, byte for byte: the piece made again from the same lo packs keeps its
    // content names.
    let names: Vec<Option<String>> = [hi_logical(LAYER, T.0, T.1), hi_logical(CLASS_LAYER, T.0, T.1)]
        .iter()
        .map(|l| out.get(l).map(str::to_string))
        .collect();
    build_hi(&mut out, &srcs, &Fixture, None, T).unwrap();
    let again: Vec<Option<String>> = [hi_logical(LAYER, T.0, T.1), hi_logical(CLASS_LAYER, T.0, T.1)]
        .iter()
        .map(|l| out.get(l).map(str::to_string))
        .collect();
    assert_eq!(names, again);
}

/// `Fixture`'s water, with (when asked) a river running east from the sea across the land, and a
/// sea channel narrower than a z8 pixel (0.3 px) running east from the sea further south.
struct WithMore {
    river: bool,
    narrow: bool,
}

/// The river's rectangle in `T` (u0, v0, u1, v1).
const RIVER: [f64; 4] = [0.45, 0.6, 0.65, 0.62];
/// The narrow channel's.
const NARROW: [f64; 4] = [0.45, 0.8, 0.9, 0.8003];

impl WaterSource for WithMore {
    fn polys(&self, z: u8, x: u32, y: u32) -> anyhow::Result<Vec<Poly>> {
        let mut out = Fixture.polys(z, x, y)?;
        if self.narrow && (6..=14).contains(&z) {
            if let Some(r) = rect(z, x, y, NARROW[0], NARROW[1], NARROW[2], NARROW[3]) {
                out.push(square(WKind::Sea, 0, r[0], r[1], r[2], r[3]));
            }
        }
        Ok(out)
    }
    fn depth_polys(&self, z: u8, x: u32, y: u32) -> anyhow::Result<Vec<Poly>> {
        let mut out = self.polys(z, x, y)?;
        if self.river && (6..=14).contains(&z) {
            if let Some(r) = rect(z, x, y, RIVER[0], RIVER[1], RIVER[2], RIVER[3]) {
                out.push(square(WKind::River, 1 << 63, r[0], r[1], r[2], r[3]));
            }
        }
        Ok(out)
    }
    fn pin(&self) -> String {
        "fixture+".into()
    }
}

/// The content names of the packs `srcs` make on `water` (the assembly's and the piece's).
fn packs(srcs: &[Source], water: &dyn WaterSource, root: &Path) -> (Out, Vec<Option<String>>) {
    let mut out = Out::unbound(&root.join("root"), &root.join("scratch")).unwrap();
    let q = (T.0 >> 3, T.1 >> 3);
    build_lo(&mut out, srcs, water, None, q).unwrap();
    build_hi(&mut out, srcs, water, None, T).unwrap();
    let names = [
        lo_logical(LAYER, q.0, q.1),
        lo_logical(CLASS_LAYER, q.0, q.1),
        hi_logical(LAYER, T.0, T.1),
        hi_logical(CLASS_LAYER, T.0, T.1),
    ]
    .iter()
    .map(|l| out.get(l).map(str::to_string))
    .collect();
    (out, names)
}

#[test]
fn rivers_and_narrow_water_without_a_value_change_nothing() {
    let d = tempfile::tempdir().unwrap();
    let srcs = sources(d.path());
    let (_, plain) = packs(&srcs, &WithMore { river: false, narrow: false }, &d.path().join("a"));
    // (No source is drawn in rivers.)
    let (_, river) = packs(&srcs, &WithMore { river: true, narrow: false }, &d.path().join("b"));
    assert_eq!(plain, river, "a river no source is drawn in");
    // (With only the lake's survey, the sea's narrow channel has no value.)
    let lake_only = &srcs[2..];
    let (_, a) = packs(lake_only, &WithMore { river: false, narrow: false }, &d.path().join("c"));
    let (_, b) = packs(lake_only, &WithMore { river: false, narrow: true }, &d.path().join("e"));
    assert!(a.iter().any(Option::is_some));
    assert_eq!(a, b, "narrow water no source has a value in");
}

/// Rectangles of `T` (kind, key, u0, v0, u1, v1), the river's as the depth's alone.
struct Rects(Vec<(WKind, u64, [f64; 4])>);

impl WaterSource for Rects {
    fn polys(&self, z: u8, x: u32, y: u32) -> anyhow::Result<Vec<Poly>> {
        Ok(self.depth_polys(z, x, y)?.into_iter().filter(|p| p.kind != WKind::River).collect())
    }
    fn depth_polys(&self, z: u8, x: u32, y: u32) -> anyhow::Result<Vec<Poly>> {
        if !(6..=14).contains(&z) {
            return Ok(Vec::new());
        }
        Ok(self
            .0
            .iter()
            .filter_map(|&(k, id, r)| {
                rect(z, x, y, r[0], r[1], r[2], r[3]).map(|q| square(k, id, q[0], q[1], q[2], q[3]))
            })
            .collect())
    }
    fn pin(&self) -> String {
        "rects".into()
    }
}

#[test]
fn a_river_meeting_the_sea_or_a_lake_changes_nothing_without_a_value() {
    let d = tempfile::tempdir().unwrap();
    let mut srcs = sources(d.path());
    // (The deep source west of x 512 of z8 alone, so the mixed pixels have no value of their own.)
    let ((c0, r0), (c1, r1)) = (at(8, 0.0, 0.0), at(8, 0.5, 1.0));
    let p = d.path().join("west.tif");
    grid_file(&p, 8, c0, r0, (c1 - c0) as u32, (r1 - r0) as u32, &|_, _| -50.0);
    srcs[0].grids[0].file = p;
    srcs[0].grids[0].bbox = window_box(8, c0, r0, (c1 - c0) as usize, (r1 - r0) as usize);
    // (z8: 1024 pixels across T. The sea to x 512.25 of z8; a lake (no survey) on to 512.5, then a
    // river, in a band; further south the sea to 512.5, then the river; further still the sea to
    // 512.25 and a river to 512.5: pixels of sea 4 + lake 4 + river 8, sea 8 + river 8 and sea 4 +
    // river 4, and at finer zooms other mixes of them.)
    let x = |px: f64| px / 1024.0;
    let base = vec![
        (WKind::Sea, 0, [0.0, 0.0, x(512.25), 0.55]),
        (WKind::Lake, 9, [x(512.25), 0.5, x(512.5), 0.55]),
        (WKind::Sea, 0, [0.0, 0.55, x(512.5), 0.65]),
        (WKind::Sea, 0, [0.0, 0.65, x(512.25), 1.0]),
    ];
    let mut with = base.clone();
    with.push((WKind::River, 1 << 63, [x(512.5), 0.5, 0.65, 0.53]));
    with.push((WKind::River, 1 << 63, [x(512.5), 0.6, 0.65, 0.62]));
    with.push((WKind::River, 1 << 63, [x(512.25), 0.7, x(512.5), 0.72]));
    let (_, a) = packs(&srcs, &Rects(base), &d.path().join("a"));
    let (_, b) = packs(&srcs, &Rects(with), &d.path().join("b"));
    assert!(a.iter().any(Option::is_some));
    assert_eq!(a, b, "rivers no source is drawn in, meeting the sea and a lake");
}

#[test]
fn a_river_takes_the_sources_drawn_in_rivers() {
    let d = tempfile::tempdir().unwrap();
    let mut srcs = sources(d.path());
    srcs[0].rivers = true;
    let (out, _) = packs(&srcs, &WithMore { river: true, narrow: false }, d.path());
    let q = (3, T.0 >> 3, T.1 >> 3);
    let (x, y, px, py) = where_(8, 0.6, 0.61);
    let t8 = tile(&out, LAYER, "lo", q, 8, x, y).expect("the river's z8 tile");
    assert!((depth_at(&t8, px, py) + 50.0).abs() < 0.6, "{}", depth_at(&t8, px, py));
    assert_eq!(class_at(&tile(&out, CLASS_LAYER, "lo", q, 8, x, y).unwrap(), px, py), C_MEASURED | (1 << 4));
    // The land beside it has none.
    let (x, y, px, py) = where_(8, 0.6, 0.7);
    assert!(tile(&out, LAYER, "lo", q, 8, x, y).is_none_or(|t| depth_at(&t, px, py) >= NODATA_MIN));
}

#[test]
fn narrow_water_keeps_a_sources_depth() {
    let d = tempfile::tempdir().unwrap();
    let srcs = sources(d.path());
    let (out, _) = packs(&srcs, &WithMore { river: false, narrow: true }, d.path());
    let q = (3, T.0 >> 3, T.1 >> 3);
    let (x, y, px, py) = where_(8, 0.7, 0.80015);
    let t8 = tile(&out, LAYER, "lo", q, 8, x, y).expect("the channel's z8 tile");
    assert!((depth_at(&t8, px, py) + 50.0).abs() < 0.6, "{}", depth_at(&t8, px, py));
    // (Without the channel, the land there has none.)
    let (plain, _) = packs(&srcs, &Fixture, &d.path().join("p"));
    assert!(tile(&plain, LAYER, "lo", q, 8, x, y).is_none_or(|t| depth_at(&t, px, py) >= NODATA_MIN));
}

#[test]
fn an_estimated_source_is_classed_so() {
    let d = tempfile::tempdir().unwrap();
    let mut srcs = sources(d.path());
    srcs[2].estimated = true;
    let (out, _) = packs(&srcs, &Fixture, d.path());
    let (x, y, px, py) = where_(11, 0.75, 0.25);
    let c = tile(&out, CLASS_LAYER, "hi", (6, T.0, T.1), 11, x, y).expect("the lake's z11 tile");
    assert_eq!(class_at(&c, px, py), C_ESTIMATED | (3 << 4));
}

/// Task #398: an assembly's and a piece's run are timed by stage (no hour of "(untimed)"): the
/// sources' files opened and windows read, the water read, composing and within it feathering
/// (and the gaps filled, which the fixture has none of), the tiles encoded, the coarse levels and the packs written.
#[test]
fn the_depth_steps_time_their_stages() {
    // (In a process of its own, this test alone: the run's timings are the process's.)
    const ALONE: &str = "SCENIC_TEST_ALONE";
    if std::env::var_os(ALONE).is_none() {
        let out = store::spawn::output(
            store::spawn::command(std::env::current_exe().unwrap())
                .args(["--exact", "depth::tests::the_depth_steps_time_their_stages", "--nocapture"])
                .env(ALONE, "1"),
        )
        .unwrap();
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success() && said.contains(" 1 passed"), "{said}{}", String::from_utf8_lossy(&out.stderr));
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let srcs = sources(d.path());
    crate::timings::reset();
    crate::timings::start("depth-lo");
    packs(&srcs, &Fixture, d.path());
    let r = crate::timings::snapshot(true).unwrap();
    let top: Vec<&str> = r.phases.iter().map(|p| p.name.as_str()).collect();
    for p in ["the basemap's water read", "composing", "tiles encoded", "z3–5 from their children", "packs written"] {
        assert!(top.contains(&p), "{p} not in {top:?}");
    }
    let composing = r.phases.iter().find(|p| p.name == "composing").unwrap();
    let sub: Vec<&str> = composing.sub.iter().map(|p| p.name.as_str()).collect();
    // (The fixture's water has no gap near data to fill: "gaps filled" isn't run.)
    for p in ["depth source files opened", "depth sources' windows read", "feathering", "stored levels scaled up"] {
        assert!(sub.contains(&p), "{p} not in composing's {sub:?}");
    }
    let opened = composing.sub.iter().find(|p| p.name == "depth source files opened").unwrap();
    assert!(opened.files > 0);
    // Nearly all of it timed.
    assert!(r.untimed_s <= 0.2 * r.wall_s + 0.05, "untimed {} of {}", r.untimed_s, r.wall_s);
    eprintln!("{}", serde_json::to_string_pretty(&r).unwrap());
}

/// Task #398: a source file's open that fails for a moment (SMB's "No such file or directory" on
/// a file that's there) is tried again; one that keeps failing fails after its waits.
#[test]
fn a_source_files_open_is_tried_again_after_a_transient_error() {
    use std::cell::Cell;
    use std::io;
    let waits = [std::time::Duration::from_millis(1); 3];
    let tries = Cell::new(0);
    let flaky = |_: &Path| -> io::Result<u32> {
        tries.set(tries.get() + 1);
        if tries.get() <= 2 {
            Err(io::Error::from_raw_os_error(libc::ENOENT))
        } else {
            Ok(7)
        }
    };
    assert_eq!(grids::open_retrying(Path::new("/nas/BlueTopo_x.low.tif"), &flaky, &waits).unwrap(), 7);
    assert_eq!(tries.get(), 3);
    tries.set(0);
    let gone = |_: &Path| -> io::Result<u32> {
        tries.set(tries.get() + 1);
        Err(io::Error::from_raw_os_error(libc::ENOENT))
    };
    let e = grids::open_retrying(Path::new("/nas/x.tif"), &gone, &waits).unwrap_err();
    assert_eq!((e.kind(), tries.get()), (io::ErrorKind::NotFound, 4));
    // And through the depth's reader: a real file opened on the readers' pool.
    let d = tempfile::tempdir().unwrap();
    let srcs = sources(d.path());
    let rd = grids::Reader::default();
    let g = &srcs[0].grids[0];
    assert!(grids::sample(&rd, &srcs[0], g.zoom, 0, 0, 16, 16).is_ok());
}

/// Task #398's measure (run by hand, read-only: nothing written to the root): a real z3 tile's
/// assembly composed up to a zoom from the NAS, its grids opened and sampled and its windows'
/// blocks read in turn (`reads` 1, as before) or at once, timed.
/// SCENIC_DEPTH_MEASURE="<root> <x>/<y> <upto zoom> <reads>"; unset, nothing.
/// Task #433: a z14 tile's water polygons are read once for its z15 and z16 descendants (a block
/// of 8 × 8 z16 tiles and its margin asks the water of 100 tiles, from 9 z14 tiles), across the
/// levels; a level's own tiles are read once each across neighbouring blocks.
#[test]
fn a_z14_tiles_water_is_read_once_for_its_finer_descendants() {
    use super::build::WaterCache;
    use super::compose::Block;
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    struct Counting(Mutex<BTreeMap<(u8, u32, u32), usize>>, Mutex<Vec<(u8, u32, u32)>>);
    impl WaterSource for Counting {
        fn polys(&self, z: u8, x: u32, y: u32) -> anyhow::Result<Vec<Poly>> {
            Fixture.polys(z, x, y)
        }
        fn depth_polys(&self, z: u8, x: u32, y: u32) -> anyhow::Result<Vec<Poly>> {
            *self.0.lock().unwrap().entry((z, x, y)).or_default() += 1;
            self.1.lock().unwrap().push((z, x, y));
            Fixture.depth_polys(z, x, y)
        }
        fn pin(&self) -> String {
            "counting".into()
        }
    }
    let counting = || Counting(Mutex::new(BTreeMap::new()), Mutex::new(Vec::new()));
    let bytes_of = |t: (u8, u32, u32)| {
        let ps = Fixture.depth_polys(t.0, t.1, t.2).unwrap();
        ps.iter().map(|p| 48 + p.rings.iter().map(|r| 24 + r.len() * 16).sum::<usize>()).sum::<usize>()
    };
    let src = counting();
    let mut wc = WaterCache::default();
    // (The z14 tiles a block and its margin of a tile touch.)
    let touched = |b: Block| -> std::collections::BTreeSet<(u32, u32)> {
        let d = b.z - 14;
        (b.ty0 - 1..=b.ty0 + b.ny).flat_map(|y| (b.tx0 - 1..=b.tx0 + b.nx).map(move |x| (x >> d, y >> d))).collect()
    };
    // (Two z16 blocks side by side at the sea's edge of T, then the first again: their water
    // tiles kept, nothing read again.)
    let (x0, y0) = (T.0 << 10, T.1 << 10);
    let blocks = [(x0 + 504, y0 + 8), (x0 + 512, y0 + 8), (x0 + 504, y0 + 8)];
    let mut expect = std::collections::BTreeSet::new();
    for (bx, by) in blocks {
        let b = Block { z: 16, tx0: bx, ty0: by, nx: 8, ny: 8 };
        expect.extend(touched(b));
        let w = wc.block(&src, b).unwrap();
        assert!(w.water.iter().any(|&w| w), "the fixture's sea is there");
    }
    let once = |reads: &BTreeMap<(u8, u32, u32), usize>, expect: &std::collections::BTreeSet<(u32, u32)>| {
        assert!(reads.keys().all(|k| k.0 == 14), "only z14 is read: {reads:?}");
        assert!(reads.values().all(|&n| n == 1), "each z14 tile once: {reads:?}");
        let got: std::collections::BTreeSet<(u32, u32)> = reads.keys().map(|k| (k.1, k.2)).collect();
        assert_eq!(&got, expect);
    };
    once(&src.0.lock().unwrap(), &expect);
    assert!(wc.polys_bytes() > 0 && wc.polys_bytes() <= super::build::POLYS_BYTES);
    // A level done: the next level's blocks over the same z14 tiles are drawn from the polygons
    // kept, read no more; new ones read once.
    wc.next_level();
    let b = Block { z: 15, tx0: x0 / 2 + 252, ty0: y0 / 2 + 4, nx: 8, ny: 8 };
    expect.extend(touched(b));
    wc.block(&src, b).unwrap();
    once(&src.0.lock().unwrap(), &expect);
    // Bounded to a byte: a block's z14 tiles are read together (task #492: once each, whatever the
    // bound) and only the last read is kept past it; the next block reads its own again.
    let src = counting();
    let mut wc = WaterCache::with_bound(1);
    // (Blocks in the sea: every tile has polygons.)
    let b = Block { z: 16, tx0: x0 + 256, ty0: y0 + 8, nx: 8, ny: 8 };
    let w1 = wc.block(&src, b).unwrap();
    let reads: usize = src.0.lock().unwrap().values().sum();
    assert_eq!(reads, touched(b).len(), "{reads} reads of {} tiles", touched(b).len());
    let last = *src.1.lock().unwrap().last().unwrap();
    assert!(bytes_of(last) > 0);
    assert_eq!(wc.polys_bytes(), bytes_of(last), "the last tile's alone");
    let w2 = wc.block(&src, b).unwrap();
    assert_eq!(w1, w2, "the same water from the tiles kept");
    assert_eq!(w1, WaterCache::default().block(&src, b).unwrap(), "and whatever the bound");
    let reads: usize = src.0.lock().unwrap().values().sum();
    assert_eq!(reads, 2 * touched(b).len(), "the block's tiles kept: nothing read again");
    // A block farther down the tile first, then one above it: the tile read last has the smallest
    // key, and the ones after it in order go too (the eviction never stops at the new one).
    let src = counting();
    let mut wc = WaterCache::with_bound(1);
    for ty in [y0 + 24, y0 + 8] {
        wc.block(&src, Block { z: 16, tx0: x0 + 256, ty0: ty, nx: 8, ny: 8 }).unwrap();
    }
    let last = *src.1.lock().unwrap().last().unwrap();
    assert!(bytes_of(last) > 0 && last.2 < (y0 + 24) >> 2, "{last:?}");
    assert_eq!(wc.polys_bytes(), bytes_of(last), "the last tile's alone, the rows below it gone");
}

/// A measure on real data, reading the NAS alone: `SCENIC_DEPTH_WATER="<root> 6/x/y <zoom> <blocks>
/// <rounds>"` reads the water of the first `blocks` blocks of z6 tile x/y's first row at `zoom`
/// (`WaterCache::block`, as a level's blocks read it, the z14 polygons kept: task #433), and as
/// before it: each block's tiles' polygons read and rasterised one by one (`polys_at`,
/// `water_tile`), `rounds` times each in turn; prints each round's time and the basemap tiles
/// read.
#[test]
fn measure_the_waters_reads() {
    use super::build::WaterCache;
    use super::compose::{Block, BLOCK};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let Ok(m) = std::env::var("SCENIC_DEPTH_WATER") else { return };
    let a: Vec<&str> = m.split_whitespace().collect();
    let root = Path::new(a[0]);
    let (x, y) = a[1].strip_prefix("6/").unwrap().split_once('/').unwrap();
    let t: (u32, u32) = (x.parse().unwrap(), y.parse().unwrap());
    let zoom: u8 = a[2].parse().unwrap();
    let blocks: u32 = a[3].parse().unwrap();
    let rounds: usize = a[4].parse().unwrap();
    struct Counting<W: WaterSource>(W, AtomicUsize);
    impl<W: WaterSource> WaterSource for Counting<W> {
        fn polys(&self, z: u8, x: u32, y: u32) -> anyhow::Result<Vec<Poly>> {
            self.0.polys(z, x, y)
        }
        fn depth_polys(&self, z: u8, x: u32, y: u32) -> anyhow::Result<Vec<Poly>> {
            self.1.fetch_add(1, Ordering::Relaxed);
            self.0.depth_polys(z, x, y)
        }
        fn pin(&self) -> String {
            self.0.pin()
        }
    }
    let scratch = tempfile::tempdir().unwrap();
    let out = Out::unbound(root, scratch.path()).unwrap();
    let water = Counting(crate::terrain_pack::open_water(&out).unwrap(), AtomicUsize::new(0));
    let n = 1u32 << (zoom - 6);
    for round in 0..rounds {
        for kept in [true, false] {
            let mut wc = WaterCache::default();
            water.1.store(0, Ordering::Relaxed);
            let t0 = std::time::Instant::now();
            let mut wet = 0usize;
            for bx in (t.0 * n..t.0 * n + n).step_by(BLOCK as usize).take(blocks as usize) {
                let b = Block { z: zoom, tx0: bx, ty0: t.1 * n, nx: BLOCK.min(t.0 * n + n - bx), ny: BLOCK };
                if kept {
                    wet += wc.block(&water, b).unwrap().water.iter().filter(|&&w| w).count();
                } else {
                    // (As before: each tile of the block and its margin read on its own.)
                    for ty in b.ty0 - 1..=b.ty0 + b.ny {
                        for tx in b.tx0 - 1..=b.tx0 + b.nx {
                            let ps = super::build::polys_at(&water, zoom, tx, ty).unwrap();
                            if !ps.is_empty() {
                                let wt = crate::terrain_water::water_tile(&ps);
                                wet += wt.sea.iter().filter(|&&s| s >= 8).count();
                            }
                        }
                    }
                }
            }
            println!(
                "round {round}, z14 polygons {}: {blocks} blocks of z{zoom} in {:.1} s, {} basemap tiles read, {wet} water pixels, {:.1} MB of z14 polygons held",
                if kept { "kept" } else { "read for each tile (as before)" },
                t0.elapsed().as_secs_f64(),
                water.1.load(Ordering::Relaxed),
                wc.polys_bytes() as f64 / 1e6
            );
        }
    }
}

/// A measure on real data, reading the NAS alone (nothing written): `SCENIC_DEPTH_MEASURE="<root>
/// x/y <upto> <reads>"` composes z3 tile x/y's assembly to zoom `upto` (`SCENIC_DEPTH_MEASURE="<root>
/// 6/x/y <upto> <reads>"`: z6 tile x/y's piece, from the NAS's lo packs) with `reads` windows read
/// at once (1: in turn, the grids too), and prints its phases, the depth files opened and read, and
/// the digest of the tiles made. `SCENIC_DEPTH_PACKED="<id>=<index>=<folder> …"` (task #452) reads
/// source `id`'s grids by a packed index (producers/depth/repack.py's) from its packs in `folder`
/// instead: the same digest is the same output. With `SCENIC_CACHE` (and `SCENIC_ROOM_TARGET_MB`)
/// set, the basemap and the packs are read from whole local copies there, as a job's are
/// (crate::localread, task #551; the copies' time is the phase "whole local copies made").
/// A packed index's feature as the build reads it (`Source::from_declared`'s, one grid).
fn grid_of(root: &Path, id: &str, f: &crate::inputs::sources::Feature) -> grids::Grid {
    let decl = "name = \"N\"\ncredit = \"C\"\nlicence = \"CC0-1.0\"\nterritory = \"world\"\n[data]\nrole = \"depth\"\nkind = \"data\"\nformat = \"depth-grid\"\nindex = \"i.geojson\"\n[data.params]\ncode = 3\nwater = \"sea\"\nlabel = \"L\"\ndatum = \"D\"\n";
    let d = crate::inputs::sources::parse(decl.as_bytes()).unwrap();
    let s = crate::inputs::sources::Source { id: id.into(), data: d.data, keyed: "k".into(), index: vec![f.clone()] };
    grids::Source::from_declared(root, &s).unwrap().grids.remove(0)
}

#[test]
fn measure_a_real_assembly() {
    let Ok(m) = std::env::var("SCENIC_DEPTH_MEASURE") else { return };
    let a: Vec<&str> = m.split_whitespace().collect();
    let root = Path::new(a[0]);
    let (z, xy) = a[1].strip_prefix("6/").map_or((3u8, a[1]), |r| (6, r));
    let (x, y) = xy.split_once('/').unwrap();
    let q: (u32, u32) = (x.parse().unwrap(), y.parse().unwrap());
    let upto: u8 = a[2].parse().unwrap();
    grids::WINDOW_READS.store(a[3].parse().unwrap(), std::sync::atomic::Ordering::Relaxed);
    grids::GRIDS_TOGETHER.store(a[3] != "1", std::sync::atomic::Ordering::Relaxed);
    // (Its open-files limit a job's, so a reader keeps as many grids open: crate::sys.)
    crate::sys::raise_open_files();
    let scratch = tempfile::tempdir().unwrap();
    let out = Out::unbound(root, scratch.path()).unwrap();
    let mut srcs = grids::declared(out.root(), &out.manifest).unwrap();
    for o in std::env::var("SCENIC_DEPTH_PACKED").unwrap_or_default().split_whitespace() {
        let v: Vec<&str> = o.split('=').collect();
        let (id, index, dir) = (v[0], Path::new(v[1]), Path::new(v[2]));
        let s = srcs.iter_mut().find(|s| s.id == id).expect("a declared depth source");
        let feats = crate::inputs::sources::parse_index(&std::fs::read(index).unwrap()).unwrap();
        let mut pin = std::collections::HashMap::new();
        let mut packed: Vec<grids::Grid> = Vec::new();
        for f in &feats {
            let g = grid_of(root, id, f);
            pin.insert(g.file.clone(), g);
        }
        for g in pin.into_values() {
            let mut g = g;
            let p = g.packed.as_mut().expect("a packed grid");
            p.pack = std::sync::Arc::new(dir.join(p.pack.file_name().unwrap()));
            packed.push(g);
        }
        // (Its grids as the index orders them: coarse first, then by file.)
        packed.sort_by(|a, b| a.zoom.cmp(&b.zoom).then(a.file.cmp(&b.file)));
        let n = packed.len();
        // (A trial's index has the grids meeting a box: those of the target, the rest kept loose.)
        let names: std::collections::HashSet<std::path::PathBuf> = packed.iter().map(|g| g.file.clone()).collect();
        let mut all: Vec<grids::Grid> = s.grids.iter().filter(|g| !names.contains(&g.file)).cloned().collect();
        all.extend(packed);
        all.sort_by(|a, b| a.zoom.cmp(&b.zoom).then(a.file.cmp(&b.file)));
        s.grids = all;
        println!("{id}: {n} grids read from packs in {}", dir.display());
    }
    crate::timings::start(if z == 3 { "depth-lo" } else { "depth" });
    let t0 = std::time::Instant::now();
    let local = crate::localread::Local::from_env(root).map(std::sync::Arc::new);
    let water = {
        let _p = crate::timings::phase("the basemap's water opened", crate::timings::Class::NasRead);
        crate::terrain_pack::open_water_with(&out, local.as_deref()).unwrap()
    };
    let io0 = grids::io_counts();
    let made = if z == 3 {
        super::build::compose_lo(&out, &srcs, &water, local.clone(), q, upto).unwrap()
    } else {
        super::build::compose_hi(&out, &srcs, &water, local.clone(), q, upto).unwrap()
    };
    let wall = t0.elapsed().as_secs_f64();
    let r = crate::timings::snapshot(true).unwrap();
    println!("{z}/{}/{} to z{upto}, reads {}: {} tiles in {wall:.1} s", q.0, q.1, a[3], made.len());
    let io = grids::io_counts();
    let mut h = Vec::new();
    for (k, (d, c)) in &made {
        h.extend_from_slice(format!("{k:?} {} {}\n", d.len(), c.len()).as_bytes());
        h.extend_from_slice(d);
        h.extend_from_slice(c);
    }
    println!(
        "  depth files opened {}, range reads {}; tiles' digest {}",
        io.0 - io0.0,
        io.1 - io0.1,
        store::naming::hash16(&h)
    );
    for p in &r.phases {
        println!("  {:<34} {:>8.1} s  n={}", p.name, p.wall_s, p.n);
        for s in &p.sub {
            println!("    {:<32} {:>8.1} s  n={}", s.name, s.wall_s, s.n);
        }
    }
    println!("  untimed {:.1} s of {:.1} s", r.untimed_s, r.wall_s);
}
