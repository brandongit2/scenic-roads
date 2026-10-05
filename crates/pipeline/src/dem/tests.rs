//! The elevation step end to end, over synthetic DEMs held in memory: each source where it serves,
//! sampled at the right pixel of the right image, the fall-through to the next source, FABDEM's
//! store (a tile downloaded once, the open sea remembered), the cache reused, Taiwan's FABDEM
//! samples taken again once the MOI DTM is there, and an interrupted run resumed.

use super::*;
use crate::fetch::{testzip::zip, MapFetch};
use crate::geotiff::testtiff::{make_images, Opts};
use crate::geotiff::write_f32;
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
    moi: PathBuf,
}

const NB: (f64, f64) = (-66.5, 46.0);
const PA: (f64, f64) = (-76.6, 40.6);
const TOKYO: (f64, f64) = (139.7, 35.7);
const TAIPEI: (f64, f64) = (121.5, 24.01);
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
        let (hrdem, _) = make_images(&[(560, 560), (280, 280), (140, 140)], Opts { tile: Some(64), compression: 5, ..Opts::default() }, hgt, Some("-32767"), &|k, x, y, _| {
            if k != 2 {
                0.0
            } else if x < 30 {
                -32767.0
            } else {
                hf.at(x as f64, y as f64)
            }
        });
        let h2 = [hgt[0], 8.0, 0.0, hgt[3], 0.0, -8.0];
        let mgt = [x0 - 1500.0, 30.0, 0.0, y0 + 1500.0, 0.0, -30.0];
        let mf = Field(50.0, 0.1, 0.2);
        let (mrdem, _) = make_images(&[(100, 100)], Opts { tile: Some(64), compression: 5, ..Opts::default() }, mgt, Some("-32767"), &|_, x, y, _| mf.at(x as f64, y as f64));
        fetch.0.insert(mrdem_url(), Some(mrdem));
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
        let (usgs, _) = make_images(&[(200, 200)], Opts { tile: Some(64), compression: 5, predictor: 3, ..Opts::default() }, tgt, Some("-999999"), &|_, x, y, _| tf.at(x as f64, y as f64));
        for v in grid(PA.0, PA.1, 2, (0.002, 0.002)) {
            let (lon, lat) = ll(v);
            let (cf, rf) = pixel(tgt, lon, lat);
            verts.push(v);
            want.push((DemSource::Usgs3dep as u8, Some(tf.at(cf, rf))));
        }
        // Every HRDEM tile the North American points fall in gets the same file (only New
        // Brunswick's points lie on it); every 3DEP tile but Pennsylvania's is missing.
        let na: Vec<u32> = (0..verts.len() as u32).collect();
        let (lon, lat): (Vec<f64>, Vec<f64>) = verts.iter().map(|&v| ll(v)).unzip();
        for (tid, bb) in hrdem_tiles().unwrap() {
            if na.iter().any(|&i| {
                let (x, y) = atlas.project(lon[i as usize], lat[i as usize]);
                x >= bb[0] && x < bb[2] && y > bb[1] && y <= bb[3]
            }) {
                fetch.0.insert(hrdem_url(&tid), Some(hrdem.clone()));
            }
        }
        for (name, _) in usgs_groups(&lon, &lat, &na) {
            fetch.0.insert(usgs_url(&name), if name == "n41w077" { Some(usgs.clone()) } else { None });
        }

        // Tokyo: GSI's 1 m lidar on the even z15 columns, else (5 m, photogrammetry: none) the 10 m.
        let png = gsi_tile();
        for v in grid(TOKYO.0, TOKYO.1, 2, (0.002, 0.002)) {
            let (lon, lat) = ll(v);
            let (fx, fy) = proj::mercator_tile(lon, lat, 15);
            let (tx, ty) = (fx.floor() as i64, fy.floor() as i64);
            let lidar = tx % 2 == 0;
            fetch.0.insert(gsi::url("dem1a_png", 15, tx, ty), lidar.then(|| png.clone()));
            for layer in ["dem5a_png", "dem5b_png", "dem5c_png"] {
                fetch.0.insert(gsi::url(layer, 15, tx, ty), None);
            }
            let (gx, gy) = proj::mercator_tile(lon, lat, 14);
            let (gtx, gty) = (gx.floor() as i64, gy.floor() as i64);
            fetch.0.insert(gsi::url("dem_png", 14, gtx, gty), Some(png.clone()));
            let (c, r) = if lidar { ((fx - tx as f64) * 256.0 - 0.5, (fy - ty as f64) * 256.0 - 0.5) } else { ((gx - gtx as f64) * 256.0 - 0.5, (gy - gty as f64) * 256.0 - 0.5) };
            let value = ((0.0..=255.0).contains(&c) && (0.0..=255.0).contains(&r)).then_some(0.01 * (1000.0 + c + 2.0 * r));
            verts.push(v);
            want.push((if lidar { DemSource::Gsi5a } else { DemSource::Gsi10 } as u8, value));
        }

        // Taipei: the MOI DTM (TWD97 / TM2 zone 121, strips, Deflate, horizontal predictor).
        let tm = proj::TransverseMercator::tm2(121.0);
        let (mx, my) = tm.project(TAIPEI.0, TAIPEI.1).unwrap();
        let ogt = [mx - 1000.0, 20.0, 0.0, my + 1000.0, 0.0, -20.0];
        let of = Field(20.0, 0.05, 0.1);
        const TWD97_TM2: &[u16] = &[1, 1, 0, 2, key::MODEL_TYPE, 0, 1, 1, key::PROJECTED_CS_TYPE, 0, 1, 3826];
        let (moi, _) = make_images(&[(100, 100)], Opts { tile: None, rows: 7, compression: 8, predictor: 2, keys: TWD97_TM2, ..Opts::default() }, ogt, Some("-32767"), &|_, x, y, _| of.at(x as f64, y as f64));
        let moi_dir = dir.path().join("moi");
        std::fs::create_dir_all(&moi_dir).unwrap();
        std::fs::write(moi_dir.join("taiwan.tif"), moi).unwrap();
        for v in grid(TAIPEI.0, TAIPEI.1, 1, (0.003, 0.003)) {
            let (lon, lat) = ll(v);
            let (x, y) = tm.project(lon, lat).unwrap();
            let (cf, rf) = pixel(ogt, x, y);
            verts.push(v);
            want.push((DemSource::Moi as u8, Some(of.at(cf, rf))));
        }

        // FABDEM: Taiwan beyond the MOI DTM from the store; Paris downloaded from its zip; the open
        // sea remembered (N53W009), or learnt (the zip without the tile, the zip that isn't there).
        let store = dir.path().join("store");
        std::fs::create_dir_all(&store).unwrap();
        let fgt = [121.48, PX, 0.0, 24.22, 0.0, -PX];
        let ff = Field(5.0, 0.01, 0.02);
        let data: Vec<f32> = (0..864u32).flat_map(|y| (0..504u32).map(move |x| ff.at(x as f64, y as f64) as f32)).collect();
        std::fs::write(store.join("N24E121_FABDEM_V1-2.tif"), write_f32(504, 864, &data, 512, fgt, 4326, Some(-9999.0)).unwrap()).unwrap();
        std::fs::write(store.join("N53W009.none"), b"").unwrap();
        let v = e7(121.6, 24.2);
        let (lon, lat) = ll(v);
        let (cf, rf) = pixel(fgt, lon, lat);
        verts.push(v);
        want.push((DemSource::Fabdem as u8, Some(ff.at(cf, rf))));
        // Paris, as Bristol's tiles are: PixelIsPoint, Deflate, horizontal predictor.
        const POINT_4326: &[u16] = &[1, 1, 0, 3, key::MODEL_TYPE, 0, 1, 2, key::RASTER_TYPE, 0, 1, 2, key::GEOGRAPHIC_TYPE, 0, 1, 4326];
        let pf = Field(35.0, 0.02, -0.03);
        let tie = [PARIS.0 - 60.0 * PX + 0.5 * PX, PX, 0.0, PARIS.1 + 60.0 * PX - 0.5 * PX, 0.0, -PX];
        let (paris, _) = make_images(&[(120, 120)], Opts { tile: Some(64), compression: 8, predictor: 2, keys: POINT_4326, ..Opts::default() }, tie, Some("-9999"), &|_, x, y, _| pf.at(x as f64, y as f64));
        let pgt = crate::geotiff::Tiff::open(Arc::new(paris.clone())).unwrap().transform(0).unwrap();
        fetch.0.insert(fabdem::zip_url("N40E000-N50E010"), Some(zip(&[("N48E002_FABDEM_V1-2.tif", &paris, false)])));
        fetch.0.insert(fabdem::zip_url("N40W030-N50W020"), None);
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
        Fixture { moi: moi_dir.join("taiwan.tif"), dir, verts, want, fetch }
    }

    fn build(&self) -> PathBuf {
        self.dir.path().join("build")
    }

    fn config(&self, moi: bool) -> Config {
        Config { build: self.build(), workers: 4, cache: self.dir.path().join("cache"), no_cache: false, moi_dtm: if moi { vec![self.moi.clone()] } else { Vec::new() }, fabdem_store: Some(self.dir.path().join("store")) }
    }

    fn outputs(&self) -> (Vec<f32>, Vec<u8>) {
        (bytemuck::pod_collect_to_vec(&std::fs::read(self.build().join("elev.f32")).unwrap()), std::fs::read(self.build().join("src.u8")).unwrap())
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

#[test]
fn every_source_where_it_serves() {
    let f = Fixture::new();
    let stats = run(&f.config(true), &f.fetch).unwrap();
    let (elev, src) = f.outputs();
    for (i, (&(s, v), (&e, &got))) in f.want.iter().zip(elev.iter().zip(&src)).enumerate() {
        assert_eq!(got, s, "vertex {i} at {:?}", f.verts[i]);
        match v {
            Some(v) => assert!((e as f64 - v).abs() < 2e-3, "vertex {i} (source {s}): {e} for {v}"),
            None if s == 0 => assert!(e.is_nan()),
            None => assert!(e.is_finite()),
        }
    }
    let count = |c: DemSource| f.want.iter().filter(|w| w.0 == c as u8).count();
    assert!(count(DemSource::Hrdem) > 10 && count(DemSource::Mrdem) > 0 && count(DemSource::Gsi5a) > 0 && count(DemSource::Gsi10) > 0, "the fixture exercises each fall-through");
    assert_eq!((stats.hrdem, stats.mrdem, stats.usgs3dep, stats.gsi5a, stats.gsi10, stats.moi, stats.fabdem, stats.missing), (count(DemSource::Hrdem), count(DemSource::Mrdem), 25, count(DemSource::Gsi5a), count(DemSource::Gsi10), 9, 10, 3));
    // The store: Paris's tile downloaded (whole, as it reads), the open sea learnt.
    let store = f.dir.path().join("store");
    assert!(crate::whole::tiff_file_whole(&store.join("N48E002_FABDEM_V1-2.tif")));
    assert!(store.join("N43E005.none").exists() && store.join("N40W030.none").exists());
    assert_eq!(std::fs::read_dir(&store).unwrap().count(), 5, "no temporary files left");
    // The cache: every vertex by key.
    let keys: Vec<u64> = bytemuck::pod_collect_to_vec(&std::fs::read(f.dir.path().join("cache/dem-cache.keys.u64")).unwrap());
    assert_eq!(keys.len(), f.verts.len());
    assert!(keys.windows(2).all(|w| w[0] <= w[1]));
    assert!(!f.build().join("dem-progress.json").exists() && !f.build().join("elev.f32.tmp").exists());
    let stats_file: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(f.build().join("dem-stats.json")).unwrap()).unwrap();
    assert_eq!(stats_file["vertices"], f.verts.len());

    // Again: every vertex from the cache but the open sea's, sampled again (from the store's
    // `.none`s, asking nothing), and Taiwan's from FABDEM (with the MOI DTM here, those are always
    // taken again).
    let first = std::fs::read(f.build().join("elev.f32")).unwrap();
    let counting = Counting { inner: &f.fetch, asked: Mutex::default(), fail: None };
    let again = run(&f.config(true), &counting).unwrap();
    assert_eq!(again.sampled_this_run, 4);
    assert_eq!(std::fs::read(f.build().join("elev.f32")).unwrap(), first);
    assert!(counting.asked.lock().unwrap().is_empty());
}

#[test]
fn a_store_this_worker_cant_write_gives_what_it_has_and_the_rest_is_read_in_place() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let store = f.dir.path().join("store");
    let listed = || {
        let mut v: Vec<String> = std::fs::read_dir(&store).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    };
    let before = listed();
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o555)).unwrap();
    let stats = run(&f.config(true), &f.fetch);
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o755)).unwrap();
    let stats = stats.unwrap();
    // The same elevations as a run that stores Paris's tile, and nothing written to the store.
    assert_eq!(listed(), before);
    let ro = f.outputs();
    let whole = Fixture::new();
    let whole_stats = run(&whole.config(true), &whole.fetch).unwrap();
    assert_eq!((stats.fabdem, stats.missing), (whole_stats.fabdem, whole_stats.missing));
    assert_eq!(ro.1, whole.outputs().1);
    assert!(ro.0.iter().zip(&whole.outputs().0).all(|(a, b)| a.to_bits() == b.to_bits()));
}

#[test]
fn taiwan_sampled_again_once_the_moi_dtm_is_there() {
    let f = Fixture::new();
    // Without the MOI DTM, Taiwan's points come from FABDEM (or nowhere, beyond its tile).
    run(&f.config(false), &f.fetch).unwrap();
    let (_, src) = f.outputs();
    let taiwan: Vec<usize> = (0..f.verts.len()).filter(|&i| in_taiwan(f.verts[i][0] as f64 * 1e-7, f.verts[i][1] as f64 * 1e-7)).collect();
    assert_eq!(taiwan.len(), 10);
    assert!(taiwan.iter().all(|&i| src[i] == DemSource::Fabdem as u8));
    // With it, those cached FABDEM samples are taken again: from the MOI DTM where it has them.
    let stats = run(&f.config(true), &f.fetch).unwrap();
    assert_eq!(stats.sampled_this_run, 13);
    let (_, src) = f.outputs();
    for &i in &taiwan {
        assert_eq!(src[i], f.want[i].0, "vertex {i}");
    }
}

#[test]
fn an_interrupted_run_resumes() {
    let f = Fixture::new();
    let paris = fabdem::zip_url("N40E000-N50E010");
    let failing = Counting { inner: &f.fetch, asked: Mutex::default(), fail: Some(paris.clone()) };
    assert!(run(&f.config(true), &failing).is_err());
    let progress: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(f.build().join("dem-progress.json")).unwrap()).unwrap();
    let done: Vec<&str> = progress["done"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert!(done.contains(&"cache") && done.contains(&"hrdem:10_3") && done.contains(&"mrdem") && done.contains(&"gsi:dem_png") && done.contains(&"moi:taiwan.tif"), "{done:?}");
    // Resumed: what was done isn't asked for again, and the outputs are a whole run's.
    let resumed = Counting { inner: &f.fetch, asked: Mutex::default(), fail: None };
    run(&f.config(true), &resumed).unwrap();
    let asked = resumed.asked.lock().unwrap().clone();
    assert!(asked.iter().all(|u| u.contains("bris.ac.uk")), "{asked:?}");
    let whole = Fixture::new();
    run(&whole.config(true), &whole.fetch).unwrap();
    assert_eq!(f.outputs().1, whole.outputs().1);
    assert!(f.outputs().0.iter().zip(&whole.outputs().0).all(|(a, b)| a.to_bits() == b.to_bits()));
}
