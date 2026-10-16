//! The fixture's sources declared (docs/inputs.md §6.8) and read by the generic sampler: today's
//! heights and sources, but in the blend bands (#143's first step, its proof in small).

use super::*;

/// A fetch that answers "none" for GSI's tiles it wasn't given (a band reaching into tiles the
/// fixture has no points in: the open sea, as GSI's server answers).
struct Lenient<'a>(&'a MapFetch);

impl Fetch for Lenient<'_> {
    fn open(&self, url: &str) -> Result<Option<Arc<dyn store::range::RangeRead>>> {
        self.0.open(url)
    }

    fn get(&self, url: &str) -> Result<Option<Vec<u8>>> {
        match self.0 .0.get(url) {
            None if url.starts_with(gsi::HOST) => Ok(None),
            _ => self.0.get(url),
        }
    }
}

/// The fixture's sources as their declarations give them, in their order.
pub fn declared_plan() -> declared::Plan {
    use declared::{Dataset, Source};
    let geo = |id: &str, code: u8, resolution: f64, crs: u16, read_resolution: Option<f64>, store: bool, files: Vec<(String, Option<[f64; 4]>)>| Dataset { id: id.into(), code, resolution, cover: None, source: Source::Geotiff { crs, read_resolution, store, files } };
    let japan = Some(vec![[122.0, 20.0, 154.0, 47.0]]);
    let tiles = |id: &str, layer: &str, zoom: u32, code: u8| Dataset { id: id.into(), code, resolution: generic::tile_resolution(zoom, 35.0), cover: japan.clone(), source: Source::Tiles { url: format!("{}/xyz/{layer}/{{z}}/{{x}}/{{y}}.png", gsi::HOST), zoom, encoding: "gsi-png".into() } };
    let hrdem: Vec<(String, Option<[f64; 4]>)> = hrdem_tiles().unwrap().into_iter().map(|(t, b)| (hrdem_url(&t), Some(b))).collect();
    let usgs = vec![(usgs_url("n41w077"), Some([-77.0, 40.0, -76.0, 41.0]))];
    let fab = |t: &str, z: &str, b: [f64; 4]| (format!("{}#{}", fabdem::zip_url(z), fabdem::member(t)), Some(b));
    declared::Plan {
        fmt: 1,
        version: "test".into(),
        tag: "t".into(),
        datasets: vec![
            tiles("gsi-1a", "dem1a_png", 15, 5),
            tiles("gsi-5a", "dem5a_png", 15, 5),
            tiles("gsi-5b", "dem5b_png", 15, 6),
            tiles("gsi-5c", "dem5c_png", 15, 6),
            geo("hrdem", 1, 8.0, 3979, Some(8.0), false, hrdem),
            tiles("gsi-10", "dem_png", 14, 7),
            geo("usgs-3dep", 2, 10.3, 4269, None, false, usgs),
            geo("mrdem", 3, 30.0, 3979, None, false, vec![(mrdem_url(), None)]),
            geo("fabdem", 4, 30.9, 4326, None, true, vec![fab("N24E121", "N20E120-N30E130", [121.0, 24.0, 122.0, 25.0]), fab("N48E002", "N40E000-N50E010", [2.0, 48.0, 3.0, 49.0])]),
        ],
    }
}

#[test]
fn declared_datasets_give_todays_heights_but_in_their_blend_bands() {
    let today = Fixture::new();
    run(&today.config(), &today.fetch).unwrap();
    let (te, ts) = today.outputs();
    let f = Fixture::new();
    // (FABDEM's stored tile where the generic store keeps it.)
    let kept = f.dir.path().join("fetched/fabdem");
    std::fs::create_dir_all(&kept).unwrap();
    std::fs::copy(f.dir.path().join("store/N24E121_FABDEM_V1-2.tif"), kept.join("N24E121_FABDEM_V1-2.tif")).unwrap();
    let plan = declared_plan();
    let lenient = Lenient(&f.fetch);
    let stats = generic::run(&f.config(), &plan, &lenient).unwrap();
    let (ge, gs) = f.outputs();
    // The blends, from the sampler itself: every vertex whose height differs is one of them.
    let lon: Vec<f64> = f.verts.iter().map(|v| v[0] as f64 * 1e-7).collect();
    let lat: Vec<f64> = f.verts.iter().map(|v| v[1] as f64 * 1e-7).collect();
    let o = generic::Opts { fetch: &lenient, store: Some(f.dir.path().join("fetched")), read_only: false, pool: None };
    let s = generic::sample(&plan, &lon, &lat, &o, &|_| {}).unwrap();
    let same = |a: f32, b: f32| a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan());
    let mut differ = 0;
    for i in 0..f.verts.len() {
        assert_eq!(gs[i], ts[i], "vertex {i}: the same source");
        assert_eq!(s.src[i], gs[i]);
        assert!(same(s.elev[i], ge[i]), "vertex {i}: the run's own height");
        if !same(ge[i], te[i]) {
            differ += 1;
            assert!(s.blended[i], "vertex {i} at {:?}: {} for {}, outside a blend band", f.verts[i], ge[i], te[i]);
        }
    }
    assert!(differ > 0 && stats.blended > 0, "the fixture has seams");
    assert_eq!(stats.missing, 3);
    // Pennsylvania's 3DEP has nothing beneath it there: unchanged.
    let pa: Vec<usize> = (0..f.verts.len()).filter(|&i| gs[i] == DemSource::Usgs3dep as u8).collect();
    assert!(!pa.is_empty() && pa.iter().all(|&i| !s.blended[i]));
    // Again: every vertex from the cache, nothing asked for.
    let counting = Counting { inner: &f.fetch, asked: Mutex::default(), fail: None };
    let again = generic::run(&f.config(), &plan, &counting).unwrap();
    assert_eq!(again.sampled_this_run, 3);
    assert_eq!(f.outputs().0.iter().map(|v| v.to_bits()).collect::<Vec<_>>(), ge.iter().map(|v| v.to_bits()).collect::<Vec<_>>());
}
