//! The pool's sampler end to end (crate::dem::generic, what the terrain's base reads), over
//! synthetic datasets held in memory and declared as today's are (docs/inputs.md §6.8): each
//! dataset where it serves, sampled at the right pixel of the right image, the fall-through to the
//! next (a fill mask's voids too), the blends at the edges, and a stored dataset read from its store
//! alone.

use super::*;
use crate::fetch::{Fetch, MapFetch};
use crate::geotiff::key;
use crate::geotiff::testtiff::{make_images, Opts};
use crate::geotiff::write_f32;
use anyhow::bail;
use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

/// Value a + b·x + c·y at pixel (x, y).
#[derive(Clone, Copy)]
struct Field(f64, f64, f64);

impl Field {
    fn at(&self, x: f64, y: f64) -> f64 {
        self.0 + self.1 * x + self.2 * y
    }
}

/// The synthetic datasets' source bytes (their declarations' `code`, as today's declare them).
#[derive(Clone, Copy, PartialEq)]
enum DemSource {
    Hrdem = 1,
    Usgs3dep = 2,
    Glo30 = 3,
    Gsi5a = 5,
    Gsi5 = 6,
    Gsi10 = 7,
}

const PX: f64 = 1.0 / 3600.0;

/// The pixel-centre position of map point (x, y) in an image with geotransform `gt`.
fn pixel(gt: [f64; 6], x: f64, y: f64) -> (f64, f64) {
    ((x - gt[0]) / gt[1] - 0.5, (y - gt[3]) / gt[5] - 0.5)
}

fn e7(lon: f64, lat: f64) -> [i32; 2] {
    [(lon * 1e7).round() as i32, (lat * 1e7).round() as i32]
}

fn grid(lon: f64, lat: f64, n: i32, step: (f64, f64)) -> Vec<[i32; 2]> {
    let mut v = Vec::new();
    for j in -n..=n {
        for i in -n..=n {
            v.push(e7(lon + i as f64 * step.0, lat + j as f64 * step.1));
        }
    }
    v
}

/// A GSI tile: 0.01·(1000 + x + 2y) m at pixel (x, y).
fn gsi_tile() -> Vec<u8> {
    let mut rgb = Vec::with_capacity(256 * 256 * 3);
    for y in 0..256u32 {
        for x in 0..256u32 {
            let v = 1000 + x + 2 * y;
            rgb.extend_from_slice(&[(v >> 16) as u8, (v >> 8) as u8, v as u8]);
        }
    }
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, 256, 256);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(&rgb).unwrap();
    out
}

/// What a vertex should get: its source, and its value (None: don't check it, a clamped edge).
type Want = (u8, Option<f64>);

struct Fixture {
    dir: tempfile::TempDir,
    verts: Vec<[i32; 2]>,
    want: Vec<Want>,
    fetch: MapFetch,
}

const NB: (f64, f64) = (-66.5, 46.0);

/// The fixture's servers (as the declarations name them).
const HRDEM_URL: &str = "https://canelevation-dem.s3.ca-central-1.amazonaws.com/hrdem-mosaic-2m/10_3-mosaic-2m-dtm.tif";
const USGS_URL: &str =
    "https://prd-tnm.s3.amazonaws.com/StagedProducts/Elevation/13/TIFF/current/n41w077/USGS_13_n41w077.tif";
const GSI: &str = "https://cyberjapandata.gsi.go.jp/xyz";
const GLO30: &str = "https://copernicus-dem-30m.s3.amazonaws.com";

fn gsi_url(layer: &str, z: u32, x: i64, y: i64) -> String {
    format!("{GSI}/{layer}/{z}/{x}/{y}.png")
}

/// A GLO-30 tile's URL (`DEM`) or its fill mask's (`FLM`), by the tile's name.
fn glo_url(tile: &str, kind: &str) -> String {
    format!("{GLO30}/{tile}/{tile}_{kind}.tif")
}
const PA: (f64, f64) = (-76.6, 40.6);
const TOKYO: (f64, f64) = (139.7, 35.7);
const PARIS: (f64, f64) = (2.35, 48.85);

impl Fixture {
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let atlas = proj::Atlas::new();
        let mut fetch = MapFetch::default();
        let mut verts: Vec<[i32; 2]> = Vec::new();
        let mut want: Vec<Want> = Vec::new();
        let ll = |v: [i32; 2]| (v[0] as f64 * 1e-7, v[1] as f64 * 1e-7);

        // New Brunswick: HRDEM's 8 m overview, nodata in its western strip, where GLO-30 serves.
        let (x0, y0) = atlas.project(NB.0, NB.1);
        let hgt = [((x0 - 560.0) / 8.0).floor() * 8.0, 2.0, 0.0, ((y0 + 560.0) / 8.0).ceil() * 8.0, 0.0, -2.0];
        let hf = Field(100.0, 0.5, 0.25);
        let (hrdem, _) = make_images(
            &[(560, 560), (280, 280), (140, 140)],
            Opts { tile: Some(64), compression: 5, ..Opts::default() },
            hgt,
            Some("-32767"),
            &|k, x, y, _| {
                if k != 2 {
                    0.0
                } else if x < 30 {
                    -32767.0
                } else {
                    hf.at(x as f64, y as f64)
                }
            },
        );
        let h2 = [hgt[0], 8.0, 0.0, hgt[3], 0.0, -8.0];
        // (GLO-30 kept in the store, as the declaration has it: one file over New Brunswick's points.)
        let store = dir.path().join("fetched/glo30");
        std::fs::create_dir_all(&store).unwrap();
        let mgt = [NB.0 - 0.1, 1.0 / 1200.0, 0.0, NB.1 + 0.1, 0.0, -1.0 / 1200.0];
        let mf = Field(50.0, 0.1, 0.2);
        let nb: Vec<f32> =
            (0..240u32).flat_map(|y| (0..240u32).map(move |x| mf.at(x as f64, y as f64) as f32)).collect();
        std::fs::write(store.join("NB_DEM.tif"), write_f32(240, 240, &nb, 256, mgt, 4326, Some(-32767.0)).unwrap())
            .unwrap();
        for v in grid(NB.0, NB.1, 2, (0.0015, 0.0012)) {
            let (lon, lat) = ll(v);
            let (x, y) = atlas.project(lon, lat);
            let (cf, rf) = pixel(h2, x, y);
            let c0 = cf.floor() as i64;
            let w = if c0 >= 30 {
                (DemSource::Hrdem as u8, Some(hf.at(cf, rf)))
            } else if cf.round_ties_even() >= 30.0 {
                (DemSource::Hrdem as u8, Some(hf.at(cf.round_ties_even(), rf.round_ties_even())))
            } else {
                let (mc, mr) = pixel(mgt, lon, lat);
                (DemSource::Glo30 as u8, Some(mf.at(mc, mr)))
            };
            verts.push(v);
            want.push(w);
        }

        // Pennsylvania: 3DEP (LZW, floating-point predictor).
        let tgt = [PA.0 - 100.0 / 10800.0, 1.0 / 10800.0, 0.0, PA.1 + 100.0 / 10800.0, 0.0, -1.0 / 10800.0];
        let tf = Field(300.0, 0.3, -0.2);
        let (usgs, _) = make_images(
            &[(200, 200)],
            Opts { tile: Some(64), compression: 5, predictor: 3, ..Opts::default() },
            tgt,
            Some("-999999"),
            &|_, x, y, _| tf.at(x as f64, y as f64),
        );
        for v in grid(PA.0, PA.1, 2, (0.002, 0.002)) {
            let (lon, lat) = ll(v);
            let (cf, rf) = pixel(tgt, lon, lat);
            verts.push(v);
            want.push((DemSource::Usgs3dep as u8, Some(tf.at(cf, rf))));
        }
        fetch.0.insert(HRDEM_URL.into(), Some(hrdem));
        fetch.0.insert(USGS_URL.into(), Some(usgs));

        // Tokyo: GSI's 1 m lidar on the even z15 columns, else (5 m, photogrammetry: none) the 10 m.
        let png = gsi_tile();
        for v in grid(TOKYO.0, TOKYO.1, 2, (0.002, 0.002)) {
            let (lon, lat) = ll(v);
            let (fx, fy) = proj::mercator_tile(lon, lat, 15);
            let (tx, ty) = (fx.floor() as i64, fy.floor() as i64);
            let lidar = tx % 2 == 0;
            fetch.0.insert(gsi_url("dem1a_png", 15, tx, ty), lidar.then(|| png.clone()));
            for layer in ["dem5a_png", "dem5b_png", "dem5c_png"] {
                fetch.0.insert(gsi_url(layer, 15, tx, ty), None);
            }
            let (gx, gy) = proj::mercator_tile(lon, lat, 14);
            let (gtx, gty) = (gx.floor() as i64, gy.floor() as i64);
            fetch.0.insert(gsi_url("dem_png", 14, gtx, gty), Some(png.clone()));
            let (c, r) = if lidar {
                ((fx - tx as f64) * 256.0 - 0.5, (fy - ty as f64) * 256.0 - 0.5)
            } else {
                ((gx - gtx as f64) * 256.0 - 0.5, (gy - gty as f64) * 256.0 - 0.5)
            };
            let value =
                ((0.0..=255.0).contains(&c) && (0.0..=255.0).contains(&r)).then_some(0.01 * (1000.0 + c + 2.0 * r));
            verts.push(v);
            want.push((if lidar { DemSource::Gsi5a } else { DemSource::Gsi10 } as u8, value));
        }

        // GLO-30, read from the store alone: Taiwan, whose fill mask makes its pixels there voids
        // (filled from another source: none here, so none); Paris; the open sea, whose tiles the
        // index doesn't list.
        let fgt = [121.48, PX, 0.0, 24.22, 0.0, -PX];
        let ff = Field(5.0, 0.01, 0.02);
        let data: Vec<f32> =
            (0..864u32).flat_map(|y| (0..504u32).map(move |x| ff.at(x as f64, y as f64) as f32)).collect();
        std::fs::write(
            store.join("N24E121_DEM.tif"),
            write_f32(504, 864, &data, 512, fgt, 4326, Some(-9999.0)).unwrap(),
        )
        .unwrap();
        let mask: Vec<f32> = (0..864u32).flat_map(|_| (0..504u32).map(|x| if x < 300 { 3.0 } else { 1.0 })).collect();
        std::fs::write(store.join("N24E121_FLM.tif"), write_f32(504, 864, &mask, 512, fgt, 4326, None).unwrap())
            .unwrap();
        // (Column 432 of 504: unmasked; column 72: masked, a void.)
        for (lon, w) in [(121.6, true), (121.5, false)] {
            let v = e7(lon, 24.2);
            let (lon, lat) = ll(v);
            let (cf, rf) = pixel(fgt, lon, lat);
            verts.push(v);
            want.push(if w { (DemSource::Glo30 as u8, Some(ff.at(cf, rf))) } else { (0, None) });
        }
        // Paris, as GLO-30's tiles are: PixelIsPoint, Deflate, horizontal predictor.
        const POINT_4326: &[u16] =
            &[1, 1, 0, 3, key::MODEL_TYPE, 0, 1, 2, key::RASTER_TYPE, 0, 1, 2, key::GEOGRAPHIC_TYPE, 0, 1, 4326];
        let pf = Field(35.0, 0.02, -0.03);
        let tie = [PARIS.0 - 60.0 * PX + 0.5 * PX, PX, 0.0, PARIS.1 + 60.0 * PX - 0.5 * PX, 0.0, -PX];
        let (paris, _) = make_images(
            &[(120, 120)],
            Opts { tile: Some(64), compression: 8, predictor: 2, keys: POINT_4326, ..Opts::default() },
            tie,
            Some("-9999"),
            &|_, x, y, _| pf.at(x as f64, y as f64),
        );
        let pgt = crate::geotiff::Tiff::open(Arc::new(paris.clone())).unwrap().transform(0).unwrap();
        std::fs::write(store.join("N48E002_DEM.tif"), &paris).unwrap();
        for v in grid(PARIS.0, PARIS.1, 1, (0.01, 0.01)) {
            let (lon, lat) = ll(v);
            let (cf, rf) = pixel(pgt, lon, lat);
            verts.push(v);
            want.push((DemSource::Glo30 as u8, Some(pf.at(cf, rf))));
        }
        for v in [e7(5.4, 43.3), e7(-8.5, 53.3), e7(-30.0, 40.0)] {
            verts.push(v);
            want.push((0, None));
        }
        // Out of order, as roads are.
        let mut order: Vec<usize> = (0..verts.len()).collect();
        order.sort_by_key(|&i| (i * 7919) % verts.len());
        let verts: Vec<[i32; 2]> = order.iter().map(|&i| verts[i]).collect();
        let want: Vec<Want> = order.iter().map(|&i| want[i]).collect();
        Fixture { dir, verts, want, fetch }
    }

    fn store(&self) -> PathBuf {
        self.dir.path().join("fetched")
    }
}

/// A fetch that counts what it's asked for, and can fail one URL.
struct Counting<'a> {
    inner: &'a MapFetch,
    asked: Mutex<Vec<String>>,
    fail: Option<String>,
}

impl Fetch for Counting<'_> {
    fn open(&self, url: &str) -> Result<Option<Arc<dyn store::range::RangeRead>>> {
        self.asked.lock().unwrap().push(url.to_string());
        if self.fail.as_deref() == Some(url) {
            bail!("{url}: no answer");
        }
        self.inner.open(url)
    }

    fn get(&self, url: &str) -> Result<Option<Vec<u8>>> {
        self.asked.lock().unwrap().push(url.to_string());
        self.inner.get(url)
    }
}

/// A fetch that answers "none" for GSI's tiles it wasn't given (a band reaching into tiles the
/// fixture has no points in: the open sea, as GSI's server answers).
struct Lenient<'a>(&'a dyn Fetch);

impl Fetch for Lenient<'_> {
    fn open(&self, url: &str) -> Result<Option<Arc<dyn store::range::RangeRead>>> {
        self.0.open(url)
    }

    fn get(&self, url: &str) -> Result<Option<Vec<u8>>> {
        match self.0.get(url) {
            Err(_) if url.starts_with(GSI) => Ok(None),
            r => r,
        }
    }
}

/// The fixture's datasets as their declarations give them (today's: docs/inputs.md §6.8), in their
/// order, each with its coverage and files.
fn declared_plan() -> declared::Plan {
    use declared::{Dataset, Source};
    let geo = |id: &str,
               code: u8,
               resolution: f64,
               cover: Option<[f64; 4]>,
               crs: u16,
               read_resolution: Option<f64>,
               store: bool,
               files: Vec<(String, Option<[f64; 4]>)>| Dataset {
        convert: None,
        id: id.into(),
        code,
        resolution,
        cover: cover.map(|c| vec![c]),
        source: Source::Geotiff { crs, read_resolution, store, files, masks: Vec::new() },
    };
    let glo =
        |tile: &str, b: [f64; 4], mask: bool| ((glo_url(tile, "DEM"), Some(b)), mask.then(|| glo_url(tile, "FLM")));
    let tiles = |id: &str, layer: &str, zoom: u32, code: u8| Dataset {
        convert: None,
        id: id.into(),
        code,
        resolution: generic::tile_resolution(zoom, 35.0),
        cover: Some(vec![[122.0, 20.0, 154.0, 47.0]]),
        source: Source::Tiles { url: format!("{GSI}/{layer}/{{z}}/{{x}}/{{y}}.png"), zoom, encoding: "gsi-png".into() },
    };
    let canada = Some([-70.0, 44.0, -63.0, 48.0]);
    declared::Plan {
        fmt: 1,
        version: "test".into(),
        tag: "0000000000000001".into(),
        datasets: vec![
            tiles("gsi-1a", "dem1a_png", 15, DemSource::Gsi5a as u8),
            tiles("gsi-5a", "dem5a_png", 15, DemSource::Gsi5a as u8),
            tiles("gsi-5b", "dem5b_png", 15, DemSource::Gsi5 as u8),
            tiles("gsi-5c", "dem5c_png", 15, DemSource::Gsi5 as u8),
            geo("hrdem", DemSource::Hrdem as u8, 8.0, canada, 3979, Some(8.0), false, vec![(HRDEM_URL.into(), None)]),
            tiles("gsi-10", "dem_png", 14, DemSource::Gsi10 as u8),
            geo(
                "usgs-3dep",
                DemSource::Usgs3dep as u8,
                10.3,
                Some([-78.0, 39.0, -75.0, 42.0]),
                4269,
                None,
                false,
                vec![(USGS_URL.into(), Some([-77.0, 40.0, -76.0, 41.0]))],
            ),
            {
                let (files, masks): (Vec<_>, Vec<_>) = [
                    glo("NB", [-67.0, 45.0, -66.0, 47.0], false),
                    glo("N24E121", [121.0, 24.0, 122.0, 25.0], true),
                    glo("N48E002", [2.0, 48.0, 3.0, 49.0], false),
                ]
                .into_iter()
                .unzip();
                Dataset {
                    convert: None,
                    id: "glo30".into(),
                    code: DemSource::Glo30 as u8,
                    resolution: 30.9,
                    cover: None,
                    source: Source::Geotiff { crs: 4326, read_resolution: None, store: true, files, masks },
                }
            },
        ],
    }
}

/// The run's outputs, and whether each vertex's height is a blend (the sampler's own word).
fn sample_fixture(f: &Fixture, store: Option<std::path::PathBuf>, fetch: &dyn Fetch) -> Result<generic::Sampled> {
    let plan = declared_plan();
    let lon: Vec<f64> = f.verts.iter().map(|v| v[0] as f64 * 1e-7).collect();
    let lat: Vec<f64> = f.verts.iter().map(|v| v[1] as f64 * 1e-7).collect();
    let o = generic::Opts { fetch: &Lenient(fetch), store, pool: None };
    generic::sample(&plan, &lon, &lat, &o, &|_| {})
}

#[test]
fn every_dataset_where_it_serves_and_the_blends_at_their_edges() {
    let f = Fixture::new();
    let s = sample_fixture(&f, Some(f.store()), &f.fetch).unwrap();
    let (elev, src, blended) = (&s.elev, &s.src, &s.blended);
    for (i, (&(c, v), (&e, &got))) in f.want.iter().zip(elev.iter().zip(src)).enumerate() {
        assert_eq!(got, c, "vertex {i} at {:?}", f.verts[i]);
        if blended[i] {
            // (Between the dataset's own height and the one beneath it: finite.)
            assert!(e.is_finite(), "vertex {i}");
            continue;
        }
        match v {
            Some(v) => assert!((e as f64 - v).abs() < 2e-3, "vertex {i} (source {c}): {e} for {v}"),
            None if c == 0 => assert!(e.is_nan()),
            None => assert!(e.is_finite()),
        }
    }
    // The seams: New Brunswick's lidar beside its no-data strip, over GLO-30. Pennsylvania's 3DEP
    // has nothing beneath it there. (Tokyo's points lie farther from its lidar columns' edges than
    // a band: tiles_blend_across_their_edges.)
    let near = |c: DemSource| (0..f.verts.len()).filter(|&i| blended[i] && src[i] == c as u8).count();
    assert!(near(DemSource::Hrdem) > 0 && near(DemSource::Usgs3dep) == 0);
    let count = |c: DemSource| f.want.iter().filter(|w| w.0 == c as u8).count();
    assert!(
        count(DemSource::Hrdem) > 10
            && count(DemSource::Glo30) > 10
            && count(DemSource::Gsi5a) > 0
            && count(DemSource::Gsi10) > 0,
        "the fixture exercises each fall-through"
    );
    assert_eq!(src.iter().filter(|&&c| c == 0).count(), 4, "the open sea's three and Taiwan's void");
    // The store: only read (its four files, nothing written or left).
    assert_eq!(std::fs::read_dir(f.store().join("glo30")).unwrap().count(), 4);
}

/// A stored dataset is never downloaded: a tile the index names and the store lacks is an error
/// naming it, nothing asked of any server for it (the index's URLs only name the tiles).
#[test]
fn a_tile_the_store_lacks_is_an_error_naming_it_and_never_fetched() {
    let f = Fixture::new();
    std::fs::remove_file(f.store().join("glo30/N48E002_DEM.tif")).unwrap();
    let counting = Counting { inner: &f.fetch, asked: Mutex::default(), fail: None };
    let e = format!("{:#}", sample_fixture(&f, Some(f.store()), &counting).unwrap_err());
    assert!(
        e.contains("glo30: tile N48E002_DEM.tif isn't in the store")
            && e.contains("scenic-build store-fetch glo30 --index"),
        "{e}"
    );
    assert!(counting.asked.lock().unwrap().iter().all(|u| !u.starts_with(GLO30)), "nothing of GLO-30 asked for");
    // (One cut short is an error too, not a download.)
    std::fs::write(f.store().join("glo30/N48E002_DEM.tif"), b"II*\0short").unwrap();
    let e = format!("{:#}", sample_fixture(&f, Some(f.store()), &counting).unwrap_err());
    assert!(e.contains("tile N48E002_DEM.tif in the store") && e.contains("isn't whole"), "{e}");
    // A store-kept dataset with no store given is refused, not read from its index's URLs.
    let e = format!("{:#}", sample_fixture(&f, None, &f.fetch).unwrap_err());
    assert!(e.contains("glo30: its files are read from the store, and no store is given"), "{e}");
}
