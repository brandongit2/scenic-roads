//! The elevation step end to end, over synthetic datasets held in memory and declared as today's
//! are (docs/inputs.md §6.8): each dataset where it serves, sampled at the right pixel of the right
//! image, the fall-through to the next, the blends at the edges, a store (a tile downloaded once),
//! the cache reused, and an interrupted run done again.

use super::*;
use crate::fetch::{Fetch, MapFetch};
use crate::geotiff::key;
use crate::geotiff::testtiff::{make_images, Opts};
use crate::geotiff::write_f32;
use anyhow::bail;
use roadcore::DemSource;
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
const MRDEM_URL: &str = "https://canelevation-dem.s3.ca-central-1.amazonaws.com/mrdem-30/mrdem-30-dtm.tif";
const USGS_URL: &str =
    "https://prd-tnm.s3.amazonaws.com/StagedProducts/Elevation/13/TIFF/current/n41w077/USGS_13_n41w077.tif";
const GSI: &str = "https://cyberjapandata.gsi.go.jp/xyz";
const BRISTOL: &str = "https://data.bris.ac.uk/datasets/s5hqmjcdj8yo2ibzi9b4ew3sn";

fn gsi_url(layer: &str, z: u32, x: i64, y: i64) -> String {
    format!("{GSI}/{layer}/{z}/{x}/{y}.png")
}

fn fabdem_url(zip: &str, tile: &str) -> String {
    format!("{BRISTOL}/{zip}_FABDEM_V1-2.zip#{tile}_FABDEM_V1-2.tif")
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

        // New Brunswick: HRDEM's 8 m overview, nodata in its western strip, where MRDEM serves.
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
        let mgt = [x0 - 1500.0, 30.0, 0.0, y0 + 1500.0, 0.0, -30.0];
        let mf = Field(50.0, 0.1, 0.2);
        let (mrdem, _) = make_images(
            &[(100, 100)],
            Opts { tile: Some(64), compression: 5, ..Opts::default() },
            mgt,
            Some("-32767"),
            &|_, x, y, _| mf.at(x as f64, y as f64),
        );
        fetch.0.insert(MRDEM_URL.into(), Some(mrdem));
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
                let (mc, mr) = pixel(mgt, x, y);
                (DemSource::Mrdem as u8, Some(mf.at(mc, mr)))
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

        // FABDEM, read from the store alone: Taiwan; Paris, stored from a tile as Bristol's are
        // (crate::dem::kept::store, as scenic-build fabdem-fetch stores it); the open sea, whose
        // tiles the index doesn't list.
        let store = dir.path().join("fetched/fabdem");
        std::fs::create_dir_all(&store).unwrap();
        let fgt = [121.48, PX, 0.0, 24.22, 0.0, -PX];
        let ff = Field(5.0, 0.01, 0.02);
        let data: Vec<f32> =
            (0..864u32).flat_map(|y| (0..504u32).map(move |x| ff.at(x as f64, y as f64) as f32)).collect();
        std::fs::write(
            store.join("N24E121_FABDEM_V1-2.tif"),
            write_f32(504, 864, &data, 512, fgt, 4326, Some(-9999.0)).unwrap(),
        )
        .unwrap();
        let v = e7(121.6, 24.2);
        let (lon, lat) = ll(v);
        let (cf, rf) = pixel(fgt, lon, lat);
        verts.push(v);
        want.push((DemSource::Fabdem as u8, Some(ff.at(cf, rf))));
        // Paris, as Bristol's tiles are: PixelIsPoint, Deflate, horizontal predictor.
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
        let dl = dir.path().join("downloaded.tif");
        std::fs::write(&dl, &paris).unwrap();
        crate::dem::kept::store(&store, "N48E002_FABDEM_V1-2.tif", &dl).unwrap();
        for v in grid(PARIS.0, PARIS.1, 1, (0.01, 0.01)) {
            let (lon, lat) = ll(v);
            let (cf, rf) = pixel(pgt, lon, lat);
            verts.push(v);
            want.push((DemSource::Fabdem as u8, Some(pf.at(cf, rf))));
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
        let build = dir.path().join("build");
        std::fs::create_dir_all(&build).unwrap();
        std::fs::write(build.join("verts.bin"), bytemuck::cast_slice(&verts)).unwrap();
        Fixture { dir, verts, want, fetch }
    }

    fn build(&self) -> PathBuf {
        self.dir.path().join("build")
    }

    fn config(&self) -> Config {
        Config {
            build: self.build(),
            workers: 4,
            cache: self.dir.path().join("cache"),
            no_cache: false,
            store: Some(self.dir.path().join("fetched")),
        }
    }

    fn outputs(&self) -> (Vec<f32>, Vec<u8>) {
        (
            bytemuck::pod_collect_to_vec(&std::fs::read(self.build().join("elev.f32")).unwrap()),
            std::fs::read(self.build().join("src.u8")).unwrap(),
        )
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
        id: id.into(),
        code,
        resolution,
        cover: cover.map(|c| vec![c]),
        source: Source::Geotiff { crs, read_resolution, store, files },
    };
    let tiles = |id: &str, layer: &str, zoom: u32, code: u8| Dataset {
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
            geo("mrdem", DemSource::Mrdem as u8, 30.0, canada, 3979, None, false, vec![(MRDEM_URL.into(), None)]),
            geo(
                "fabdem",
                DemSource::Fabdem as u8,
                30.9,
                None,
                4326,
                None,
                true,
                vec![
                    (fabdem_url("N20E120-N30E130", "N24E121"), Some([121.0, 24.0, 122.0, 25.0])),
                    (fabdem_url("N40E000-N50E010", "N48E002"), Some([2.0, 48.0, 3.0, 49.0])),
                ],
            ),
        ],
    }
}

/// The run's outputs, and whether each vertex's height is a blend (the sampler's own word).
fn run_fixture(f: &Fixture, cfg: &Config, fetch: &dyn Fetch) -> Result<(generic::Stats, Vec<bool>)> {
    let plan = declared_plan();
    let stats = generic::run(cfg, &plan, &Lenient(fetch))?;
    let lon: Vec<f64> = f.verts.iter().map(|v| v[0] as f64 * 1e-7).collect();
    let lat: Vec<f64> = f.verts.iter().map(|v| v[1] as f64 * 1e-7).collect();
    let o = generic::Opts { fetch: &Lenient(fetch), store: cfg.store.clone(), pool: None };
    let s = generic::sample(&plan, &lon, &lat, &o, &|_| {})?;
    Ok((stats, s.blended))
}

#[test]
fn every_dataset_where_it_serves_and_the_blends_at_their_edges() {
    let f = Fixture::new();
    let (stats, blended) = run_fixture(&f, &f.config(), &f.fetch).unwrap();
    let (elev, src) = f.outputs();
    let mut blends = 0;
    for (i, (&(s, v), (&e, &got))) in f.want.iter().zip(elev.iter().zip(&src)).enumerate() {
        assert_eq!(got, s, "vertex {i} at {:?}", f.verts[i]);
        if blended[i] {
            // (Between the dataset's own height and the one beneath it: finite.)
            blends += 1;
            assert!(e.is_finite(), "vertex {i}");
            continue;
        }
        match v {
            Some(v) => assert!((e as f64 - v).abs() < 2e-3, "vertex {i} (source {s}): {e} for {v}"),
            None if s == 0 => assert!(e.is_nan()),
            None => assert!(e.is_finite()),
        }
    }
    assert_eq!(stats.blended, blends);
    // The seams: New Brunswick's lidar beside its no-data strip, over MRDEM. Pennsylvania's 3DEP
    // has nothing beneath it there. (Tokyo's points lie farther from its lidar columns' edges than
    // a band: tiles_blend_across_their_edges.)
    let near = |c: DemSource| (0..f.verts.len()).filter(|&i| blended[i] && src[i] == c as u8).count();
    assert!(near(DemSource::Hrdem) > 0 && near(DemSource::Usgs3dep) == 0);
    let count = |c: DemSource| f.want.iter().filter(|w| w.0 == c as u8).count();
    assert!(
        count(DemSource::Hrdem) > 10
            && count(DemSource::Mrdem) > 0
            && count(DemSource::Gsi5a) > 0
            && count(DemSource::Gsi10) > 0,
        "the fixture exercises each fall-through"
    );
    assert_eq!(
        (
            stats.datasets["hrdem"],
            stats.datasets["mrdem"],
            stats.datasets["usgs-3dep"],
            stats.datasets["fabdem"],
            stats.missing
        ),
        (count(DemSource::Hrdem), count(DemSource::Mrdem), 25, 10, 3)
    );
    // The store: only read (its two tiles, nothing written or left).
    let store = f.dir.path().join("fetched/fabdem");
    assert_eq!(std::fs::read_dir(&store).unwrap().count(), 2);
    // The cache: every vertex by key.
    let keys: Vec<u64> =
        bytemuck::pod_collect_to_vec(&std::fs::read(f.dir.path().join("cache/dem-cache.keys.u64")).unwrap());
    assert_eq!(keys.len(), f.verts.len());
    assert!(keys.windows(2).all(|w| w[0] <= w[1]));
    assert!(!f.build().join("dem-progress.json").exists() && !f.build().join("elev.f32.tmp").exists());
    let stats_file: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(f.build().join("dem-stats.json")).unwrap()).unwrap();
    assert_eq!(stats_file["vertices"], f.verts.len());

    // Again: every vertex from the cache but the open sea's, sampled again (no file holds them:
    // asking nothing).
    let first = std::fs::read(f.build().join("elev.f32")).unwrap();
    let counting = Counting { inner: &f.fetch, asked: Mutex::default(), fail: None };
    let again = generic::run(&f.config(), &declared_plan(), &counting).unwrap();
    assert_eq!(again.sampled_this_run, 3);
    assert_eq!(std::fs::read(f.build().join("elev.f32")).unwrap(), first);
    assert!(counting.asked.lock().unwrap().is_empty());
}

/// Area builds never download FABDEM: a tile the index names and the store lacks is an error
/// naming it, nothing asked of any server for it (the index's Bristol URLs only name the tiles).
#[test]
fn a_tile_the_store_lacks_is_an_error_naming_it_and_never_fetched() {
    let f = Fixture::new();
    std::fs::remove_file(f.dir.path().join("fetched/fabdem/N48E002_FABDEM_V1-2.tif")).unwrap();
    let counting = Counting { inner: &f.fetch, asked: Mutex::default(), fail: None };
    let e = format!("{:#}", generic::run(&f.config(), &declared_plan(), &Lenient(&counting)).unwrap_err());
    assert!(
        e.contains("fabdem: tile N48E002_FABDEM_V1-2.tif isn't in the store")
            && e.contains("scenic-build fabdem-fetch"),
        "{e}"
    );
    assert!(counting.asked.lock().unwrap().iter().all(|u| !u.contains("FABDEM")), "nothing of FABDEM asked for");
    // (One cut short is an error too, not a download.)
    std::fs::write(f.dir.path().join("fetched/fabdem/N48E002_FABDEM_V1-2.tif"), b"II*\0short").unwrap();
    let e = format!("{:#}", generic::run(&f.config(), &declared_plan(), &Lenient(&counting)).unwrap_err());
    assert!(e.contains("tile N48E002_FABDEM_V1-2.tif in the store") && e.contains("isn't whole"), "{e}");
    // A store-kept dataset with no store given is refused, not read from its index's URLs.
    let e = format!(
        "{:#}",
        generic::run(&Config { store: None, ..f.config() }, &declared_plan(), &Lenient(&f.fetch)).unwrap_err()
    );
    assert!(e.contains("fabdem: its files are read from the store, and no store is given"), "{e}");
}

#[test]
fn an_interrupted_run_is_done_again() {
    let f = Fixture::new();
    let failing = Counting { inner: &f.fetch, asked: Mutex::default(), fail: Some(USGS_URL.into()) };
    assert!(generic::run(&f.config(), &declared_plan(), &Lenient(&failing)).is_err());
    let progress: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(f.build().join("dem-progress.json")).unwrap()).unwrap();
    assert_eq!(progress["done"], serde_json::json!(["cache"]));
    // Again: the outputs are a whole run's.
    generic::run(&f.config(), &declared_plan(), &Lenient(&f.fetch)).unwrap();
    let whole = Fixture::new();
    run_fixture(&whole, &whole.config(), &whole.fetch).unwrap();
    assert_eq!(f.outputs().1, whole.outputs().1);
    assert!(f.outputs().0.iter().zip(&whole.outputs().0).all(|(a, b)| a.to_bits() == b.to_bits()));
}
