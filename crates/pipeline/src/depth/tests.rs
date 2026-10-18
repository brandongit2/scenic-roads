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
            label: format!("S{code}"),
            datum: "MSL".into(),
            notice: None,
            grids: vec![SGrid {
                bbox: window_box(zoom, c0, r0, (c1 - c0) as usize, (r1 - r0) as usize),
                zoom,
                file: p,
                zoom_file: None,
                low_file: None,
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
    let n_lo = build_lo(&mut out, &srcs, &Fixture, q).unwrap();
    assert!(
        n_lo > 0
            && out.get(&lo_logical(LAYER, q.0, q.1)).is_some()
            && out.get(&lo_logical(CLASS_LAYER, q.0, q.1)).is_some()
    );
    let n_hi = build_hi(&mut out, &srcs, &Fixture, T).unwrap();
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
    build_hi(&mut out, &srcs, &Fixture, T).unwrap();
    let again: Vec<Option<String>> = [hi_logical(LAYER, T.0, T.1), hi_logical(CLASS_LAYER, T.0, T.1)]
        .iter()
        .map(|l| out.get(l).map(str::to_string))
        .collect();
    assert_eq!(names, again);
}
