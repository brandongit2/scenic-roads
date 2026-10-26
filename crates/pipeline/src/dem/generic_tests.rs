//! The sampler's own rules: the blend, and footprints.

use super::*;

#[test]
fn a_blend_is_continuous_and_exact_past_its_band() {
    assert_eq!(blend(10.0, 20.0, 0.0, 40.0), 20.0);
    assert_eq!(blend(10.0, 20.0, 40.0, 40.0), 10.0);
    assert_eq!(blend(10.0, 20.0, 1e9, 40.0), 10.0);
    assert_eq!(blend(10.0, 20.0, 20.0, 40.0), 15.0);
    let a = blend(10.0, 20.0, 39.999, 40.0);
    assert!((a - 10.0).abs() < 1e-3);
    assert_eq!(smoothstep(-1.0), 0.0);
    assert_eq!(smoothstep(2.0), 1.0);
}

/// A 256 × 256 Terrarium tile of one height.
fn flat(h: f64) -> Vec<u8> {
    let v = h + 32768.0;
    let px = [(v / 256.0).floor() as u8, (v.floor() as i64 % 256) as u8, ((v - v.floor()) * 256.0) as u8];
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, 256, 256);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(&px.repeat(256 * 256)).unwrap();
    out
}

/// The map's tiles, and none for any other.
struct NoneElse(crate::fetch::MapFetch);

impl Fetch for NoneElse {
    fn open(&self, url: &str) -> Result<Option<Arc<dyn RangeRead>>> {
        self.0.open(url)
    }

    fn get(&self, url: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.0 .0.get(url).cloned().flatten())
    }
}

#[test]
fn tiles_blend_across_their_edges() {
    use super::super::declared::{Dataset, Plan, Source};
    let z = 10;
    let tiles = |id: &str, code: u8, url: &str| Dataset {
        convert: None,
        id: id.into(),
        code,
        resolution: tile_resolution(z, 0.0),
        cover: None,
        source: Source::Tiles { url: url.into(), zoom: z, encoding: "terrarium".into() },
    };
    let plan = Plan {
        fmt: 1,
        version: "t".into(),
        tag: "1".into(),
        datasets: vec![
            tiles("fine", 1, "https://a.org/{z}/{x}/{y}.png"),
            tiles("coarse", 2, "https://b.org/{z}/{x}/{y}.png"),
        ],
    };
    // The fine dataset has tile 512/511 alone (lon 0 to 0.35°, just north of the equator); the
    // coarse one has it and its neighbour east.
    let mut f = crate::fetch::MapFetch::default();
    f.0.insert("https://a.org/10/512/511.png".into(), Some(flat(100.0)));
    f.0.insert("https://b.org/10/512/511.png".into(), Some(flat(50.0)));
    f.0.insert("https://b.org/10/513/511.png".into(), Some(flat(50.0)));
    let fetch = NoneElse(f);
    let o = Opts { fetch: &fetch, store: None };
    let w = 360.0 / 1024.0;
    // Its middle, 100 m inside its east edge, at the edge itself, and past it.
    let lon = [w / 2.0, w - 100.0 / M_PER_DEG, w - 1e-9, w + 0.01];
    let lat = [0.1; 4];
    let s = sample(&plan, &lon, &lat, &o, &|_| {}).unwrap();
    assert_eq!(s.elev[0], 100.0);
    assert!(!s.blended[0]);
    assert!(s.blended[1] && s.elev[1] > 50.0 && s.elev[1] < 100.0, "{:?}", s.elev);
    assert!(s.blended[2] && (s.elev[2] - 50.0).abs() < 1.0, "at the edge, the coarse one's: {:?}", s.elev);
    assert_eq!((s.elev[3], s.src[3]), (50.0, 2));
    assert_eq!(&s.src[..3], &[1, 1, 1]);
}

#[test]
fn footprints_are_half_open() {
    let b = [0.0, 0.0, 1.0, 1.0];
    assert!(in_box(&b, 0.0, 1.0) && !in_box(&b, 1.0, 0.5) && !in_box(&b, 0.5, 0.0));
    assert_eq!(stored_name("https://e.org/z.zip#N48E002_X.tif"), "N48E002_X.tif");
    assert_eq!(stored_name("https://e.org/a/b.tif"), "b.tif");
}

#[test]
fn a_pyramids_tile_not_256_pixels_is_refused() {
    let png = |n: u32| {
        let mut out = Vec::new();
        let mut e = png::Encoder::new(&mut out, n, n);
        e.set_color(png::ColorType::Rgb);
        e.set_depth(png::BitDepth::Eight);
        e.write_header().unwrap().write_image_data(&[128u8, 0, 0].repeat((n * n) as usize)).unwrap();
        out
    };
    let mut f = crate::fetch::MapFetch::default();
    f.0.insert("https://e.org/3/1/1.png".into(), Some(png(256)));
    f.0.insert("https://e.org/3/2/1.png".into(), Some(png(512)));
    let o = Opts { fetch: &f, store: None };
    let t = TileSet {
        url: "https://e.org/{z}/{x}/{y}.png",
        zoom: 3,
        encoding: "terrarium",
        o: &o,
        clean: Mutex::new(HashMap::new()),
        memo: Arc::new(TileMemo::new(TILE_MEMO_BYTES)),
    };
    assert_eq!(t.tile(1, 1).unwrap().unwrap().0, 256);
    assert!(t.tile(2, 1).unwrap_err().to_string().contains("512 × 512"));
}

/// The distance to a file's invalid area, as the scan finds it, computed here over the raster
/// itself for every point: a hole inside a block, a strip across blocks, the file's own edges,
/// and clean blocks (where the sampler takes the band's full distance without reading a pixel).
#[test]
fn the_distance_to_the_invalid_area_is_the_scans_whatever_the_shortcut() {
    use super::super::declared::{Dataset, Plan, Source};
    use crate::geotiff::write_f32;
    const PX: f64 = 1.0 / 3600.0;
    let (w, h) = (1200usize, 1100usize);
    let gt = [10.0, PX, 0.0, 50.0, 0.0, -PX];
    let nd = -9999.0f32;
    let mut data: Vec<f32> =
        (0..h).flat_map(|y| (0..w).map(move |x| 100.0 + 0.01 * x as f32 + 0.02 * y as f32)).collect();
    // A hole in block (0, 0), a strip of no data down blocks (1, *), and one past the nodata floor.
    for y in 100..103 {
        for x in 100..103 {
            data[y * w + x] = nd;
        }
    }
    for y in 0..h {
        for x in 700..703 {
            data[y * w + x] = nd;
        }
    }
    data[900 * w + 300] = -5000.0;
    let fine = write_f32(w as u32, h as u32, &data, 256, gt, 4326, Some(nd as f64)).unwrap();
    let cgt = [9.0, 3.0 * PX, 0.0, 51.0, 0.0, -3.0 * PX];
    let coarse = write_f32(2000, 2000, &vec![50.0f32; 2000 * 2000], 512, cgt, 4326, Some(nd as f64)).unwrap();
    let mut f = crate::fetch::MapFetch::default();
    f.0.insert("https://a.org/fine.tif".into(), Some(fine));
    f.0.insert("https://b.org/coarse.tif".into(), Some(coarse));
    let o = Opts { fetch: &f, store: None };
    let fine_box = [10.0, 50.0 - h as f64 * PX, 10.0 + w as f64 * PX, 50.0];
    let geo = |id: &str, code: u8, res: f64, url: &str, b: [f64; 4]| Dataset {
        convert: None,
        id: id.into(),
        code,
        resolution: res,
        cover: None,
        source: Source::Geotiff {
            crs: 4326,
            read_resolution: None,
            store: false,
            files: vec![(url.into(), Some(b))],
            masks: Vec::new(),
        },
    };
    let (rf, rc) = (PX * M_PER_DEG, 3.0 * PX * M_PER_DEG);
    let ds_fine = geo("fine", 1, rf, "https://a.org/fine.tif", fine_box);
    let ds_coarse = geo("coarse", 2, rc, "https://b.org/coarse.tif", [9.0, 45.0, 15.0, 51.0]);
    let plan = |ds: Vec<Dataset>| Plan { fmt: 1, version: "t".into(), tag: "1".into(), datasets: ds };
    // Points: a grid over the file and a little past it, plus rings round the hole and the strip.
    let (mut lon, mut lat) = (Vec::new(), Vec::new());
    for j in -3..=110 {
        for i in -3..=120 {
            lon.push(10.0 + (i as f64 * 10.0 + 0.37) * PX);
            lat.push(50.0 - (j as f64 * 10.0 + 0.61) * PX);
        }
    }
    for (cx, cy) in [(101.0, 101.0), (701.0, 500.0), (300.0, 900.0), (1199.0, 1099.0), (0.0, 0.0)] {
        for k in 0..40 {
            let a = k as f64 * 0.157;
            let r = 1.0 + k as f64 * 0.9;
            lon.push(10.0 + (cx + 0.5 + r * a.cos()) * PX);
            lat.push(50.0 - (cy + 0.5 + r * a.sin()) * PX);
        }
    }
    let both = sample(&plan(vec![ds_fine.clone(), ds_coarse.clone()]), &lon, &lat, &o, &|_| {}).unwrap();
    let a = sample(&plan(vec![ds_fine.clone()]), &lon, &lat, &o, &|_| {}).unwrap();
    let b = sample(&plan(vec![ds_coarse.clone()]), &lon, &lat, &o, &|_| {}).unwrap();
    let band = 4.0 * rc;
    let mut blends = 0;
    for i in 0..lon.len() {
        if both.src[i] != 1 {
            assert!(!both.blended[i] && both.src[i] == a.src[i].max(b.src[i]), "point {i}");
            continue;
        }
        // The scan: the nearest invalid pixel (no data, below the floor, or past the file's edge)
        // within the band, in metres, as GeoSet::distance reckons it.
        let (mx, my) = (PX * M_PER_DEG * lat[i].to_radians().cos(), PX * M_PER_DEG);
        let cf = (lon[i] - gt[0]) / gt[1] - 0.5;
        let rfl = (lat[i] - gt[3]) / gt[5] - 0.5;
        let (rx, ry) = ((band / mx).ceil() as i64 + 1, (band / my).ceil() as i64 + 1);
        let (c0, r0) = (cf.floor() as i64, rfl.floor() as i64);
        let mut best = band;
        for r in r0 - ry..=r0 + 1 + ry {
            for c in c0 - rx..=c0 + 1 + rx {
                let d =
                    (((c as f64 - cf).abs() - 0.5).max(0.0) * mx).hypot(((r as f64 - rfl).abs() - 0.5).max(0.0) * my);
                if d >= best {
                    continue;
                }
                let inside = c >= 0 && r >= 0 && (c as usize) < w && (r as usize) < h;
                if !inside || !valid(data[r as usize * w + c as usize], Some(nd as f64)) {
                    best = d;
                }
            }
        }
        let want = if best < band { blend(a.elev[i], b.elev[i], best, band) } else { a.elev[i] };
        assert_eq!(
            (both.elev[i].to_bits(), both.blended[i]),
            (want.to_bits(), best < band),
            "point {i} at pixel ({cf:.1}, {rfl:.1}): distance {best}"
        );
        blends += (best < band) as usize;
    }
    assert!(blends > 100 && blends < lon.len() / 2, "{blends} blends of {}", lon.len());
}

#[test]
fn sampling_says_increasing_fractions_to_one_per_chunk() {
    use super::super::declared::{Dataset, Plan, Source};
    let z = 10;
    let ds = |id: &str, code: u8, url: &str| Dataset {
        convert: None,
        id: id.into(),
        code,
        resolution: tile_resolution(z, 0.0),
        cover: None,
        source: Source::Tiles { url: url.into(), zoom: z, encoding: "terrarium".into() },
    };
    let plan = Plan {
        fmt: 1,
        version: "t".into(),
        tag: "1".into(),
        datasets: vec![
            ds("fine", 1, "https://a.org/{z}/{x}/{y}.png"),
            ds("coarse", 2, "https://b.org/{z}/{x}/{y}.png"),
        ],
    };
    let mut f = crate::fetch::MapFetch::default();
    f.0.insert("https://a.org/10/512/511.png".into(), Some(flat(100.0)));
    f.0.insert("https://b.org/10/512/511.png".into(), Some(flat(50.0)));
    let fetch = NoneElse(f);
    let o = Opts { fetch: &fetch, store: None };
    let n = 100_000;
    let w = 360.0 / 1024.0;
    let lon: Vec<f64> = (0..n).map(|i| w * (0.1 + 0.8 * i as f64 / n as f64)).collect();
    let lat = vec![0.1; n];
    let said = Mutex::new(Vec::new());
    let s = sample(&plan, &lon, &lat, &o, &|f| said.lock().unwrap().push(f)).unwrap();
    assert!(s.src.iter().all(|&c| c == 1) && s.elev.iter().all(|e| e.is_finite()));
    let said = said.into_inner().unwrap();
    assert!(said.len() >= 6, "a line per chunk and per blend step: {said:?}");
    assert!(said.windows(2).all(|w| w[0] <= w[1]), "never backwards: {said:?}");
    assert!(said.iter().all(|f| (0.0..=1.0).contains(f)));
    assert_eq!(said.last(), Some(&1.0), "it reaches 1: {said:?}");
    assert!(said.iter().filter(|&&f| f > 0.0 && f < 1.0).count() >= 3, "moves within the sampling: {said:?}");
}

/// A 256 × 256 Terrarium tile of height 10 with no data (−32768) where `hole` says.
fn holed(hole: impl Fn(usize, usize) -> bool) -> Vec<u8> {
    let v = 10.0 + 32768.0;
    let data = [(v / 256.0) as u8, (v as i64 % 256) as u8, 0];
    let mut px = Vec::with_capacity(256 * 256 * 3);
    for r in 0..256 {
        for c in 0..256 {
            px.extend_from_slice(if hole(c, r) { &[0, 0, 0] } else { &data });
        }
    }
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, 256, 256);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(&px).unwrap();
    out
}

/// A pyramid's distances are what a scan of each point's whole band finds, to the bit: across
/// tiles with holes, a tile the server hasn't, a clean tile, and the world's west edge.
#[test]
fn a_pyramids_distance_is_the_whole_bands_scan() {
    let z = 6;
    let n = 1i64 << z;
    let mut seed = 7u64;
    let mut rnd = move || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut f = crate::fetch::MapFetch::default();
    let url = |x: i64, y: i64| format!("https://e.org/{z}/{x}/{y}.png");
    // (Tiles 0–2 × 30–32: holes of each shape; 1/31 missing; 2/30 clean.)
    for ty in 30..=32i64 {
        for tx in 0..=2i64 {
            let png = match (tx, ty) {
                (1, 31) => None,
                (2, 30) => Some(flat(10.0)),
                _ => {
                    let (a, b) = ((tx * 37 + ty * 11) as usize % 200, (tx * 5 + ty * 29) as usize % 200);
                    Some(holed(move |c, r| {
                        (c.abs_diff(a) < 6 && r.abs_diff(b) < 9) || (c * 7 + r * 13) % 97 == 0 || r == (a + 40) % 256
                    }))
                }
            };
            f.0.insert(url(tx, ty), png);
        }
    }
    let fetch = NoneElse(f);
    let o = Opts { fetch: &fetch, store: None };
    let t = TileSet {
        url: "https://e.org/{z}/{x}/{y}.png",
        zoom: z,
        encoding: "terrarium",
        o: &o,
        clean: Mutex::new(HashMap::new()),
        memo: Arc::new(TileMemo::new(TILE_MEMO_BYTES)),
    };
    // Points across those tiles, bands up to ~60 pixels.
    let (mut lon, mut lat, mut radius) = (Vec::new(), Vec::new(), Vec::new());
    for _ in 0..3000 {
        let (fx, fy) = (rnd() * 3.0, 30.0 + rnd() * 3.0);
        lon.push(fx / n as f64 * 360.0 - 180.0);
        let m = std::f64::consts::PI * (1.0 - 2.0 * fy / n as f64);
        lat.push(m.sinh().atan().to_degrees());
        radius.push(tile_resolution(z, 0.0) * (1.0 + rnd() * 60.0));
    }
    let sel: Vec<u32> = (0..lon.len() as u32).collect();
    let said = Mutex::new(Vec::new());
    let got = t.distance(&sel, &lon, &lat, &radius, &|f| said.lock().unwrap().push(f)).unwrap();
    let said = said.into_inner().unwrap();
    assert!(said.len() >= 5 && said.contains(&1.0), "a share per tile's points: {said:?}");
    // The scan: every pixel of the band.
    let mut tiles: HashMap<(i64, i64), Option<Vec<f32>>> = HashMap::new();
    let mut ok = |c: i64, r: i64| -> bool {
        let (tx, ty) = (c.div_euclid(256), r.div_euclid(256));
        if tx < 0 || ty < 0 || tx >= n || ty >= n {
            return false;
        }
        let tile = tiles.entry((tx, ty)).or_insert_with(|| t.tile(tx, ty).unwrap().map(|(_, _, a)| a));
        tile.as_ref().is_some_and(|a| valid(a[r.rem_euclid(256) as usize * 256 + c.rem_euclid(256) as usize], None))
    };
    let mut near = 0;
    for j in 0..lon.len() {
        let (fx, fy) = mercator_tile(lon[j], lat[j], z);
        let pm = tile_resolution(z, lat[j]);
        let (gx, gy) = (fx * 256.0 - 0.5, fy * 256.0 - 0.5);
        let rp = (radius[j] / pm).ceil() as i64 + 1;
        let (c0, r0) = (gx.floor() as i64, gy.floor() as i64);
        let mut best = radius[j];
        for r in r0 - rp..=r0 + 1 + rp {
            for c in c0 - rp..=c0 + 1 + rp {
                let d = ((c as f64 - gx).abs() - 0.5).max(0.0).hypot(((r as f64 - gy).abs() - 0.5).max(0.0)) * pm;
                if d < best && !ok(c, r) {
                    best = d;
                }
            }
        }
        near += (best < radius[j]) as usize;
        assert_eq!(got[j].to_bits(), best.to_bits(), "point {j}: {} against the scan's {best}", got[j]);
    }
    assert!(near > 1000 && near < 2900, "{near} of 3000 points near the invalid area");
}

/// A fetch that counts each URL's requests and the most at once, each taking a while.
struct Counting {
    inner: NoneElse,
    asked: Mutex<HashMap<String, usize>>,
    now: std::sync::atomic::AtomicUsize,
    most: std::sync::atomic::AtomicUsize,
}

impl Counting {
    fn new(f: crate::fetch::MapFetch) -> Counting {
        Counting { inner: NoneElse(f), asked: Default::default(), now: Default::default(), most: Default::default() }
    }
}

impl Fetch for Counting {
    fn open(&self, url: &str) -> Result<Option<Arc<dyn RangeRead>>> {
        self.inner.open(url)
    }

    fn get(&self, url: &str) -> Result<Option<Vec<u8>>> {
        use std::sync::atomic::Ordering::SeqCst;
        *self.asked.lock().unwrap().entry(url.to_string()).or_default() += 1;
        let at = self.now.fetch_add(1, SeqCst) + 1;
        self.most.fetch_max(at, SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(3));
        self.now.fetch_sub(1, SeqCst);
        self.inner.get(url)
    }
}

/// Task #379: a run fetches each tile once (its sampling, the distances and the blends' second
/// heights share them), many at once ahead of the sampling (more than the pool's threads), and the
/// heights are those of a run on one thread, to the bit.
#[test]
fn a_runs_tiles_are_fetched_once_and_together_and_the_heights_are_the_same() {
    use super::super::declared::{Dataset, Plan, Source};
    let z = 6;
    let n = 1i64 << z;
    let ds = |id: &str, code: u8, host: &str| Dataset {
        convert: None,
        id: id.into(),
        code,
        resolution: tile_resolution(z, 0.0),
        cover: None,
        source: Source::Tiles {
            url: format!("https://{host}/{{z}}/{{x}}/{{y}}.png"),
            zoom: z,
            encoding: "terrarium".into(),
        },
    };
    let plan = Plan {
        fmt: 1,
        version: "t".into(),
        tag: "1".into(),
        datasets: vec![ds("a", 1, "a.org"), ds("b", 2, "b.org"), ds("c", 3, "c.org")],
    };
    // (Tiles 0–7 × 24–31: a's with holes, some missing; b's with other holes; c's whole.)
    let mut f = crate::fetch::MapFetch::default();
    for ty in 24..32i64 {
        for tx in 0..8i64 {
            let (p, q) = ((tx * 37 + ty * 11) as usize % 200, (tx * 5 + ty * 29) as usize % 200);
            if (tx + ty) % 5 != 0 {
                let a = holed(move |c, r| c.abs_diff(p) < 30 && r.abs_diff(q) < 40);
                f.0.insert(format!("https://a.org/{z}/{tx}/{ty}.png"), Some(a));
            }
            f.0.insert(format!("https://b.org/{z}/{tx}/{ty}.png"), Some(holed(move |c, r| (c + r) % 61 == 0)));
            f.0.insert(format!("https://c.org/{z}/{tx}/{ty}.png"), Some(flat(5.0)));
        }
    }
    let mut seed = 11u64;
    let mut rnd = move || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let (mut lon, mut lat) = (Vec::new(), Vec::new());
    for _ in 0..20_000 {
        let (fx, fy) = (0.2 + rnd() * 7.6, 24.2 + rnd() * 7.6);
        lon.push(fx / n as f64 * 360.0 - 180.0);
        lat.push((std::f64::consts::PI * (1.0 - 2.0 * fy / n as f64)).sinh().atan().to_degrees());
    }
    let one = NoneElse(crate::fetch::MapFetch(f.0.clone()));
    let want = sample(&plan, &lon, &lat, &Opts { fetch: &one, store: None }, &|_| {}).unwrap();
    let counting = Counting::new(f);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    let got = pool.install(|| sample(&plan, &lon, &lat, &Opts { fetch: &counting, store: None }, &|_| {})).unwrap();
    let bits = |s: &Sampled| s.elev.iter().map(|e| e.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&got), bits(&want));
    assert_eq!((&got.src, &got.blended), (&want.src, &want.blended));
    let blends = want.blended.iter().filter(|b| **b).count();
    assert!(blends > 100 && want.src.contains(&2) && want.src.contains(&3), "{blends} blends");
    let asked = counting.asked.into_inner().unwrap();
    assert!(asked.keys().any(|u| u.starts_with("https://c.org")), "the blends' second heights read c's tiles");
    let again: Vec<_> = asked.iter().filter(|(_, &k)| k > 1).collect();
    assert!(again.is_empty(), "fetched more than once: {again:?}");
    let most = counting.most.into_inner();
    assert!(most > 2, "at most {most} tiles fetched at once, no more than the pool's threads");
}

/// The memo lets its oldest tiles go past its size, and fetches one again when asked after.
#[test]
fn the_tile_memo_lets_its_oldest_go() {
    let mut f = crate::fetch::MapFetch::default();
    for i in 0..4 {
        f.0.insert(format!("https://m.org/{i}"), Some(vec![i as u8; 1000]));
    }
    let counting = Counting::new(f);
    // (Room for two: 0 and 1 kept, 0 read again from the memo; 2 lets 0 go, so it's fetched
    // again, letting 1 go; 3 lets 2 go.)
    let memo = TileMemo::new(2500);
    for i in [0, 1, 0, 2, 0, 3] {
        assert_eq!(memo.get(&format!("https://m.org/{i}"), &counting).unwrap().unwrap()[0], i as u8);
    }
    let asked = counting.asked.into_inner().unwrap();
    assert_eq!(asked["https://m.org/0"], 2);
    assert_eq!((asked["https://m.org/1"], asked["https://m.org/2"], asked["https://m.org/3"]), (1, 1, 1));
}

/// A failed fetch leaves nothing in the memo: the next ask fetches again.
#[test]
fn a_failed_fetch_leaves_no_slot() {
    struct Fails;
    impl Fetch for Fails {
        fn open(&self, _: &str) -> Result<Option<Arc<dyn RangeRead>>> {
            anyhow::bail!("no")
        }
        fn get(&self, _: &str) -> Result<Option<Vec<u8>>> {
            anyhow::bail!("no")
        }
    }
    let memo = TileMemo::new(2500);
    assert!(memo.get("https://m.org/x", &Fails).is_err());
    assert!(memo.inner.lock().unwrap().map.is_empty());
    let mut f = crate::fetch::MapFetch::default();
    f.0.insert("https://m.org/x".into(), Some(vec![7]));
    assert_eq!(memo.get("https://m.org/x", &NoneElse(f)).unwrap().unwrap()[0], 7);
}

/// Each dataset's heights are brought to EGM2008 before the blend: the first's and the second's
/// by their own conversions, so a seam blends converted heights.
#[test]
fn heights_are_converted_before_the_blend() {
    use super::super::declared::{Dataset, Plan, Source};
    use super::super::geoid::conversion;
    let z = 10;
    // (A grid of one value over 4° around the tiles.)
    let grid =
        |v: f32| crate::geotiff::write_f32(4, 4, &[v; 16], 16, [-2.0, 1.0, 0.0, 2.0, 0.0, -1.0], 4326, None).unwrap();
    let decl = |id: &str, from: &str| {
        let t = format!(
            "name = \"N\"\ncredit = \"C\"\nlicence = \"CC0-1.0\"\nterritory = \"world\"\n[data]\nrole = \"geoid\"\nkind = \"data\"\n\
             format = \"geoid-grid\"\nurl = \"https://g.org/{id}.tif\"\n[data.params]\nfrom = \"{from}\"\nto = \"EGM2008\""
        );
        let d = crate::inputs::sources::parse(t.as_bytes()).unwrap();
        crate::inputs::sources::Source { id: id.into(), data: d.data, keyed: id.into(), index: Vec::new() }
    };
    let geoid = vec![decl("up", "R"), decl("down", "S")];
    let tiles = |id: &str, code: u8, url: &str, from: &str| Dataset {
        convert: conversion(from, &geoid).unwrap(),
        id: id.into(),
        code,
        resolution: tile_resolution(z, 0.0),
        cover: None,
        source: Source::Tiles { url: url.into(), zoom: z, encoding: "terrarium".into() },
    };
    let plan = Plan {
        fmt: 1,
        version: "t".into(),
        tag: "1".into(),
        datasets: vec![
            tiles("fine", 1, "https://a.org/{z}/{x}/{y}.png", "R"),
            tiles("coarse", 2, "https://b.org/{z}/{x}/{y}.png", "S"),
        ],
    };
    let mut f = crate::fetch::MapFetch::default();
    f.0.insert("https://a.org/10/512/511.png".into(), Some(flat(100.0)));
    f.0.insert("https://b.org/10/512/511.png".into(), Some(flat(50.0)));
    f.0.insert("https://b.org/10/513/511.png".into(), Some(flat(50.0)));
    f.0.insert("https://b.org/10/526/511.png".into(), Some(flat(50.0)));
    f.0.insert("https://g.org/up.tif".into(), Some(grid(2.0)));
    f.0.insert("https://g.org/down.tif".into(), Some(grid(-3.0)));
    let fetch = NoneElse(f);
    let o = Opts { fetch: &fetch, store: None };
    let w = 360.0 / 1024.0;
    let lon = [w / 2.0, w - 100.0 / M_PER_DEG, w + 0.01, 5.0];
    let lat = [0.1; 4];
    let s = sample(&plan, &lon, &lat, &o, &|_| {}).unwrap();
    assert_eq!(s.elev[0], 102.0);
    assert_eq!(s.elev[2], 47.0);
    assert!(s.blended[1] && s.elev[1] > 47.0 && s.elev[1] < 102.0, "{:?}", s.elev);
    // Where no grid has a value: the height as sampled, counted.
    assert_eq!((s.elev[3], s.unconverted), (50.0, 1));
    assert_eq!(s.unconverted_seam, 0);
    // The dataset beneath without a grid value in the band: the seam mixing references, counted.
    let mut plan = plan;
    plan.datasets[1].convert = conversion("T", &[decl("far", "T")]).unwrap();
    let mut f = crate::fetch::MapFetch::default();
    for (u, h) in [("a.org/10/512/511", 100.0), ("b.org/10/512/511", 50.0), ("b.org/10/513/511", 50.0)] {
        f.0.insert(format!("https://{u}.png"), Some(flat(h)));
    }
    f.0.insert("https://g.org/up.tif".into(), Some(grid(2.0)));
    f.0.insert(
        "https://g.org/far.tif".into(),
        Some(crate::geotiff::write_f32(4, 4, &[1.0; 16], 16, [40.0, 1.0, 0.0, 2.0, 0.0, -1.0], 4326, None).unwrap()),
    );
    let fetch = NoneElse(f);
    let o = Opts { fetch: &fetch, store: None };
    let s = sample(&plan, &lon[..2], &lat[..2], &o, &|_| {}).unwrap();
    assert!(s.blended[1] && s.unconverted_seam == 1 && s.unconverted == 0, "{s:?}");
}

/// A GeoTIFF's reads, each taking a while: how many times each range was read, and the most at
/// once.
struct SlowReads {
    inner: Arc<dyn RangeRead>,
    asked: Arc<Mutex<HashMap<(u64, usize), usize>>>,
    now: Arc<std::sync::atomic::AtomicUsize>,
    most: Arc<std::sync::atomic::AtomicUsize>,
}

impl RangeRead for SlowReads {
    fn len(&self) -> Result<u64, store::IoError> {
        self.inner.len()
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, store::IoError> {
        use std::sync::atomic::Ordering::SeqCst;
        *self.asked.lock().unwrap().entry((off, len)).or_default() += 1;
        let at = self.now.fetch_add(1, SeqCst) + 1;
        self.most.fetch_max(at, SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(2));
        self.now.fetch_sub(1, SeqCst);
        self.inner.read_at(off, len)
    }
}

/// Task #397: a GeoTIFF dataset's blocks are read ahead of their sampling, many at once (more
/// than the pool's threads), each once, and the heights are those of a run on one thread with
/// none read ahead, to the bit.
#[test]
fn a_geotiffs_blocks_are_read_ahead_once_and_the_heights_are_the_same() {
    use super::super::declared::{Dataset, Plan, Source};
    use crate::geotiff::write_f32;
    const PX: f64 = 1.0 / 3600.0;
    let (w, h) = (2048usize, 1536usize);
    let gt = [10.0, PX, 0.0, 50.0, 0.0, -PX];
    let nd = -9999.0f32;
    let mut data: Vec<f32> =
        (0..h).flat_map(|y| (0..w).map(move |x| 100.0 + 0.013 * x as f32 + 0.021 * y as f32)).collect();
    for y in 600..700 {
        for x in 900..1000 {
            data[y * w + x] = nd;
        }
    }
    let fine = write_f32(w as u32, h as u32, &data, 256, gt, 4326, Some(nd as f64)).unwrap();
    let cgt = [9.0, 3.0 * PX, 0.0, 51.0, 0.0, -3.0 * PX];
    let coarse = write_f32(2000, 2000, &vec![50.0f32; 2000 * 2000], 512, cgt, 4326, Some(nd as f64)).unwrap();
    let mut f = crate::fetch::MapFetch::default();
    f.0.insert("https://a.org/fine.tif".into(), Some(fine));
    f.0.insert("https://b.org/coarse.tif".into(), Some(coarse));
    let geo = |id: &str, code: u8, res: f64, url: &str, b: [f64; 4]| Dataset {
        convert: None,
        id: id.into(),
        code,
        resolution: res,
        cover: None,
        source: Source::Geotiff {
            crs: 4326,
            read_resolution: None,
            store: false,
            files: vec![(url.into(), Some(b))],
            masks: Vec::new(),
        },
    };
    let fine_box = [10.0, 50.0 - h as f64 * PX, 10.0 + w as f64 * PX, 50.0];
    let plan = Plan {
        fmt: 1,
        version: "t".into(),
        tag: "1".into(),
        datasets: vec![
            geo("fine", 1, PX * M_PER_DEG, "https://a.org/fine.tif", fine_box),
            geo("coarse", 2, 3.0 * PX * M_PER_DEG, "https://b.org/coarse.tif", [9.0, 45.0, 15.0, 51.0]),
        ],
    };
    // (Enough points for several chunks: CHUNK_MIN each.)
    let mut seed = 7u64;
    let mut rnd = move || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let (mut lon, mut lat) = (Vec::new(), Vec::new());
    for _ in 0..(CHUNK_MIN * 5) {
        lon.push(10.0 + (rnd() * 2100.0 - 20.0) * PX);
        lat.push(50.0 - (rnd() * 1580.0 - 20.0) * PX);
    }
    let want = sample(&plan, &lon, &lat, &Opts { fetch: &f, store: None }, &|_| {}).unwrap();
    struct Slow {
        inner: crate::fetch::MapFetch,
        asked: Arc<Mutex<HashMap<(u64, usize), usize>>>,
        now: Arc<std::sync::atomic::AtomicUsize>,
        most: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl Fetch for Slow {
        fn open(&self, url: &str) -> Result<Option<Arc<dyn RangeRead>>> {
            let Some(r) = self.inner.open(url)? else { return Ok(None) };
            if !url.contains("fine") {
                return Ok(Some(r));
            }
            Ok(Some(Arc::new(SlowReads {
                inner: r,
                asked: self.asked.clone(),
                now: self.now.clone(),
                most: self.most.clone(),
            })))
        }
        fn get(&self, url: &str) -> Result<Option<Vec<u8>>> {
            self.inner.get(url)
        }
    }
    let slow = Slow { inner: f, asked: Default::default(), now: Default::default(), most: Default::default() };
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    let got = pool.install(|| sample(&plan, &lon, &lat, &Opts { fetch: &slow, store: None }, &|_| {})).unwrap();
    let bits = |s: &Sampled| s.elev.iter().map(|e| e.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&got), bits(&want));
    assert_eq!((&got.src, &got.blended), (&want.src, &want.blended));
    assert!(want.blended.iter().any(|b| *b) && want.src.contains(&2), "blends and the coarse file's heights");
    let asked = slow.asked.lock().unwrap();
    let blocks: Vec<_> = asked.iter().filter(|(k, _)| k.1 >= 1024).collect();
    assert!(blocks.len() >= 40, "{} blocks read", blocks.len());
    let again: Vec<_> = blocks.iter().filter(|(_, &n)| n > 1).collect();
    assert!(again.is_empty(), "read more than once: {again:?}");
    let most = slow.most.load(std::sync::atomic::Ordering::SeqCst);
    assert!(most > 2, "at most {most} reads at once, no more than the pool's threads");
}

/// Task #445: a GeoTIFF dataset in the points' own CRS reads their `lon` and `lat` as given, no
/// copy of them per dataset (16 bytes a point each: a dense unit's 85 M points, 1.4 GB a dataset).
#[test]
fn a_geotiff_dataset_in_degrees_keeps_no_copy_of_the_points() {
    use super::super::declared::{Dataset, Source};
    let f = crate::fetch::MapFetch::default();
    let o = Opts { fetch: &f, store: None };
    let ds = |crs: u16| Dataset {
        convert: None,
        id: "d".into(),
        code: 1,
        resolution: 30.0,
        cover: None,
        source: Source::Geotiff { crs, read_resolution: None, store: false, files: Vec::new(), masks: Vec::new() },
    };
    let (lon, lat) = (vec![-80.0, -79.5], vec![43.0, 44.0]);
    let cache = Arc::new(Mutex::new(BlockCache::new(1 << 20)));
    let memo = Arc::new(TileMemo::new(1 << 20));
    let cover = Cover::of(None);
    for crs in [4326, 4269] {
        let d = ds(crs);
        let s = Set::new(&d, &o, &cache, &memo).unwrap();
        assert!(matches!(s.reader(&lon, &lat, &cover), Reader::Geo(_, None)), "EPSG:{crs}");
    }
    // (One in another CRS: its points projected, into its own two columns.)
    let d = ds(3979);
    let s = Set::new(&d, &o, &cache, &memo).unwrap();
    let r = s.reader(&lon, &lat, &cover);
    let Reader::Geo(_, Some((x, y))) = &r else { panic!("projected") };
    assert_eq!((x.len(), y.len()), (2, 2));
    assert!(x.iter().chain(y).all(|v| v.is_finite() && v.abs() > 1000.0), "{x:?} {y:?}");
    let own = Some((x.clone(), y.clone()));
    let (nx, ny) = native(&own, &lon, &lat);
    assert_eq!((nx, ny), (&x[..], &y[..]));
    assert_eq!(native(&None, &lon, &lat), (&lon[..], &lat[..]));
}

/// Task #397 follow-up: the reading ahead goes no further than a quarter of the cache holds, so
/// it can't push the chunk being sampled out of it.
#[test]
fn the_reading_ahead_is_held_to_a_quarter_of_the_cache() {
    let mb = 1 << 20;
    assert_eq!(ahead_for(CACHE_BYTES, mb), GEO_AHEAD.min(CACHE_BYTES / mb / 4));
    assert_eq!(ahead_for(32 * mb, mb), 8);
    assert_eq!(ahead_for(64 * mb, 4 * mb), 4);
    assert_eq!(ahead_for(mb, mb), 0, "a cache of one block: none ahead");
    assert_eq!(ahead_for(1 << 40, 1), GEO_AHEAD);
}

/// A 256 × 256 Terrarium tile sloping by column and row, its slope and base by `k`.
fn ramp(k: usize) -> Vec<u8> {
    let mut px = Vec::with_capacity(256 * 256 * 3);
    for r in 0..256usize {
        for c in 0..256usize {
            let v = 32768.0 + (k % 97) as f64 * 3.0 + c as f64 * 0.25 + r as f64 * (1 + k % 5) as f64 * 0.125;
            px.extend_from_slice(&[
                (v / 256.0).floor() as u8,
                (v.floor() as i64 % 256) as u8,
                ((v - v.floor()) * 256.0) as u8,
            ]);
        }
    }
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, 256, 256);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(&px).unwrap();
    out
}

/// A pyramid of 8 × 6 ramps at zoom 8, one in 13
/// missing, and points across them.
fn ramps() -> (crate::fetch::MapFetch, Vec<f64>, Vec<f64>) {
    let z = 8;
    let n = 1i64 << z;
    let mut f = crate::fetch::MapFetch::default();
    for ty in 80..86i64 {
        for tx in 100..108i64 {
            let k = (tx * 40 + ty) as usize;
            f.0.insert(format!("https://r.org/{z}/{tx}/{ty}.png"), (k % 13 != 0).then(|| ramp(k)));
        }
    }
    let mut seed = 7u64;
    let mut rnd = move || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let (mut lon, mut lat) = (Vec::new(), Vec::new());
    for _ in 0..3_000 {
        let (fx, fy) = (100.0 + rnd() * 8.0, 80.0 + rnd() * 6.0);
        lon.push(fx / n as f64 * 360.0 - 180.0);
        lat.push((std::f64::consts::PI * (1.0 - 2.0 * fy / n as f64)).sinh().atan().to_degrees());
    }
    (f, lon, lat)
}

/// The ramps' pyramid read through `o`, with a memo of `cap` bytes.
fn ramp_set<'a>(o: &'a Opts<'a>, cap: usize) -> TileSet<'a> {
    TileSet {
        url: "https://r.org/{z}/{x}/{y}.png",
        zoom: 8,
        encoding: "terrarium",
        o,
        clean: Mutex::new(HashMap::new()),
        memo: Arc::new(TileMemo::new(cap)),
    }
}

/// Task #426: the rolling fetches give each point the height the old way gave it (its tile
/// fetched, decoded and sampled on its own), to the bit, with the memo's room ample and with
/// almost none (the fetching held to the earliest unsampled tiles), and the progress moves within
/// the call, ending at all of it.
#[test]
fn rolling_tile_fetches_give_the_old_heights_to_the_bit() {
    let (f, lon, lat) = ramps();
    let fetch = NoneElse(f);
    let sel: Vec<u32> = (0..lon.len() as u32).collect();
    // (The old way, by the letter: each point's tile, decoded, sampled bilinearly.)
    let one = Opts { fetch: &fetch, store: None };
    let plain = ramp_set(&one, TILE_MEMO_BYTES);
    let want: Vec<u32> = sel
        .iter()
        .map(|&i| {
            let (fx, fy) = mercator_tile(lon[i as usize], lat[i as usize], 8);
            let (tx, ty) = (fx.floor() as i64, fy.floor() as i64);
            match plain.tile(tx, ty).unwrap() {
                None => f32::NAN.to_bits(),
                Some((w, h, a)) => {
                    bilinear(&a, w, h, (fx - tx as f64) * 256.0 - 0.5, (fy - ty as f64) * 256.0 - 0.5).to_bits()
                }
            }
        })
        .collect();
    assert!(want.iter().any(|v| f32::from_bits(*v).is_nan()) && want.iter().any(|v| f32::from_bits(*v).is_finite()));
    let pool = rayon::ThreadPoolBuilder::new().num_threads(3).build().unwrap();
    for cap in [TILE_MEMO_BYTES, 64 << 10] {
        let o = Opts { fetch: &fetch, store: None };
        let t = ramp_set(&o, cap);
        let said = Mutex::new(Vec::new());
        let got = pool.install(|| t.sample(&sel, &lon, &lat, &|f| said.lock().unwrap().push(f))).unwrap();
        let got: Vec<u32> = got.iter().map(|v| v.to_bits()).collect();
        assert_eq!(got, want, "a memo of {cap} bytes");
        let said = said.into_inner().unwrap();
        assert!(said.len() >= 20, "the progress said {} times", said.len());
        assert_eq!(said.iter().copied().fold(0.0, f64::max), 1.0);
    }
}

/// Task #426: one tile that takes long holds up its own points alone: every other tile is
/// fetched and sampled meanwhile (a batch's barrier held them all), and the call ends once it's
/// in, the heights those of a run on one thread.
#[test]
fn one_slow_tile_does_not_stall_the_others() {
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    let (f, lon, lat) = ramps();
    // (The first tile in the sampling's order.)
    let slow_url = "https://r.org/8/100/80.png";
    assert!(f.0.get(slow_url).is_some_and(|t| t.is_some()));
    struct Slow {
        inner: NoneElse,
        url: &'static str,
        /// The share of the points sampled, as said; whether the slow tile was let go by it.
        share: Mutex<f64>,
        cv: Condvar,
        by_progress: AtomicBool,
    }
    impl Fetch for Slow {
        fn open(&self, url: &str) -> Result<Option<Arc<dyn RangeRead>>> {
            self.inner.open(url)
        }
        fn get(&self, url: &str) -> Result<Option<Vec<u8>>> {
            if url == self.url {
                // (Held until nine tenths of the points are sampled, or 180 s: a stall; long, so a loaded Mac does not fail it.)
                let g = self.share.lock().unwrap();
                let wait = std::time::Duration::from_secs(180);
                let (g, t) = self.cv.wait_timeout_while(g, wait, |s| *s < 0.9).unwrap();
                self.by_progress.store(!t.timed_out() && *g >= 0.9, SeqCst);
            }
            self.inner.get(url)
        }
    }
    let slow = Slow {
        inner: NoneElse(crate::fetch::MapFetch(f.0.clone())),
        url: slow_url,
        share: Mutex::new(0.0),
        cv: Condvar::new(),
        by_progress: AtomicBool::new(false),
    };
    let pool = rayon::ThreadPoolBuilder::new().num_threads(3).build().unwrap();
    let o = Opts { fetch: &slow, store: None };
    let t = ramp_set(&o, TILE_MEMO_BYTES);
    let sel: Vec<u32> = (0..lon.len() as u32).collect();
    let tell = |f: f64| {
        let mut g = slow.share.lock().unwrap();
        *g = g.max(f);
        slow.cv.notify_all();
    };
    let got = pool.install(|| t.sample(&sel, &lon, &lat, &tell)).unwrap();
    assert!(slow.by_progress.load(SeqCst), "the other tiles waited on the slow one");
    let one = NoneElse(f);
    let o1 = Opts { fetch: &one, store: None };
    let want = ramp_set(&o1, TILE_MEMO_BYTES).sample(&sel, &lon, &lat, &|_| {}).unwrap();
    let bits = |v: &[f32]| v.iter().map(|e| e.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&got), bits(&want));
}

/// Task #426 review: a tile's fetch that panics is a failed fetch: the sampling ends in an error
/// naming the tile, not waiting forever on it.
#[test]
fn a_tile_fetch_that_panics_ends_the_sampling_in_an_error() {
    let (f, lon, lat) = ramps();
    struct Panics(NoneElse);
    impl Fetch for Panics {
        fn open(&self, url: &str) -> Result<Option<Arc<dyn RangeRead>>> {
            self.0.open(url)
        }
        fn get(&self, url: &str) -> Result<Option<Vec<u8>>> {
            if url.ends_with("/103/82.png") {
                panic!("a bad tile");
            }
            self.0.get(url)
        }
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let fetch = Panics(NoneElse(f));
        let pool = rayon::ThreadPoolBuilder::new().num_threads(3).build().unwrap();
        let o = Opts { fetch: &fetch, store: None };
        let sel: Vec<u32> = (0..lon.len() as u32).collect();
        let r = pool.install(|| ramp_set(&o, TILE_MEMO_BYTES).sample(&sel, &lon, &lat, &|_| {})).map(|_| ());
        tx.send(r.map_err(|e| format!("{e:#}"))).unwrap();
    });
    let r = rx.recv_timeout(std::time::Duration::from_secs(60)).expect("the sampling hung");
    let e = r.unwrap_err();
    assert!(e.contains("103/82.png") && e.contains("panicked: a bad tile"), "{e}");
}

/// A plan of many files finds a point's files by its grid: those a scan of every file finds, in
/// the plan's order (footprints of 1°, one of the world, one without a footprint).
#[test]
fn many_files_are_found_by_their_grid_as_by_a_scan() {
    let mut files: Vec<(String, Option<[f64; 4]>)> = Vec::new();
    for la in 40..50 {
        for lo in -10..10 {
            files.push((format!("{la} {lo}"), Some([lo as f64, la as f64, lo as f64 + 1.0, la as f64 + 1.0])));
        }
    }
    files.insert(37, ("world".into(), Some([-180.0, -90.0, 180.0, 90.0])));
    files.insert(5, ("any".into(), None));
    files.push(("again".into(), Some([0.0, 45.0, 1.0, 46.0])));
    let g = FileGrid::of(&files);
    assert!(files.len() > FileGrid::FROM && g.side == 1.0);
    let scan = |x: f64, y: f64| -> Vec<usize> {
        (0..files.len()).filter(|&k| files[k].1.is_none_or(|b| in_box(&b, x, y))).collect()
    };
    let grid = |x: f64, y: f64| -> Vec<usize> {
        g.near(x, y).iter().copied().filter(|&k| files[k].1.is_none_or(|b| in_box(&b, x, y))).collect()
    };
    for (x, y) in [(0.5, 45.5), (0.0, 46.0), (1.0, 45.0), (-10.0, 40.5), (9.99, 49.99), (50.0, 0.0), (-0.0, 40.0)] {
        assert_eq!(grid(x, y), scan(x, y), "({x}, {y})");
    }
    assert_eq!(grid(0.5, 45.5).len(), 4, "its own, the world's, the one without, the other");
    assert!(g.near(f64::NAN, 1.0).is_empty());
}

/// The descriptors this process holds on files under `dir` (macOS: by F_GETPATH).
#[cfg(target_os = "macos")]
fn fds_under(dir: &std::path::Path) -> usize {
    let dir = dir.canonicalize().unwrap();
    (0..store::cachefile::open_files_limit() as i32)
        .filter(|&fd| {
            let mut b = [0u8; libc::PATH_MAX as usize];
            // SAFETY: F_GETPATH writes at most PATH_MAX bytes into `b`.
            if unsafe { libc::fcntl(fd, libc::F_GETPATH, b.as_mut_ptr()) } != 0 {
                return false;
            }
            let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
            std::path::Path::new(std::str::from_utf8(&b[..n]).unwrap_or("")).starts_with(&dir)
        })
        .count()
}

/// A run over the world's GLO-30 tiles (25 Oct) kept every tile and its fill mask open
/// (the sets' `opened`, `opened_masks`) until "Too many open files". Sampled over many stored tiles
/// and masks, a run holds no more of their descriptors than store::range::plain_open_cap (plus
/// those mid-read, one a thread), and its heights are those of a run holding them all, to the bit.
#[cfg(target_os = "macos")]
#[test]
fn sampling_many_stored_tiles_holds_a_bounded_number_of_files() {
    use super::super::declared::{Dataset, Plan, Source};
    use crate::geotiff::write_f32;
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("glo");
    std::fs::create_dir_all(&dir).unwrap();
    // (6 × 5 tiles of 0.1°, 100 px each way, each with a mask: 60 files.)
    let (cols, rows, n) = (6usize, 5usize, 100usize);
    let mut files = Vec::new();
    let mut masks = Vec::new();
    for c in 0..cols {
        for r in 0..rows {
            let (w, nn) = (10.0 + 0.1 * c as f64, 46.0 - 0.1 * r as f64);
            let gt = [w, 0.001, 0.0, nn, 0.0, -0.001];
            let e: Vec<f32> = (0..n * n).map(|p| (100 * (c * rows + r) + p % n + p / n) as f32).collect();
            let m = vec![1.0f32; n * n];
            let name = format!("T{c}_{r}");
            let tif = |v: &[f32]| write_f32(n as u32, n as u32, v, 32, gt, 4326, None).unwrap();
            std::fs::write(dir.join(format!("{name}_DEM.tif")), tif(&e)).unwrap();
            std::fs::write(dir.join(format!("{name}_FLM.tif")), tif(&m)).unwrap();
            files.push((format!("https://e.org/{name}/{name}_DEM.tif"), Some([w, nn - 0.1, w + 0.1, nn])));
            masks.push(Some(format!("https://e.org/{name}/{name}_FLM.tif")));
        }
    }
    let plan = Plan {
        fmt: 1,
        version: "t".into(),
        tag: "1".into(),
        datasets: vec![Dataset {
            convert: None,
            id: "glo".into(),
            code: 3,
            resolution: 78.0,
            cover: None,
            source: Source::Geotiff { crs: 4326, read_resolution: None, store: true, files, masks },
        }],
    };
    let mut seed = 11u64;
    let mut rnd = move || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let (mut lon, mut lat) = (Vec::new(), Vec::new());
    for _ in 0..4000 {
        lon.push(10.0 + rnd() * 0.1 * cols as f64);
        lat.push(46.0 - rnd() * 0.1 * rows as f64);
    }
    let f = crate::fetch::MapFetch::default();
    let threads = 2;
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
    let o = Opts { fetch: &f, store: Some(store::nasfs::NasPath::local_for_test(d.path().to_path_buf())) };
    let run = |cap: usize| {
        let _cap = store::range::cap_plain_opens(cap);
        let s = Sampler::new(&plan, &o).unwrap();
        let got = pool.install(|| s.sample(&lon, &lat, &|_| {})).unwrap();
        let held = fds_under(d.path());
        drop(s);
        (got, held)
    };
    // (Every tile held, as before the fix, open at the end; masks are read whole once and closed.)
    let (want, all) = run(1_000_000);
    let (got, held) = run(8);
    eprintln!("held: {all} with no cap, {held} with a cap of 8");
    assert_eq!(all, cols * rows, "all held open");
    assert!(held <= 8 + threads, "{held} held, past the cap");
    let bits = |s: &Sampled| s.elev.iter().map(|e| e.to_bits()).collect::<Vec<_>>();
    assert_eq!(bits(&got), bits(&want));
    assert_eq!((&got.src, &got.blended), (&want.src, &want.blended));
    assert!(want.elev.iter().all(|e| e.is_finite()));
    assert_eq!(fds_under(d.path()), 0, "none once the sampler's gone");
}

#[test]
fn by_runs_orders_as_a_full_sort() {
    // Rows of points across two files and back, some filtered out, keys repeating apart.
    let mut sel = Vec::new();
    let mut keys = Vec::new();
    for i in 0..4000u32 {
        if i % 7 == 3 {
            continue;
        }
        let (row, col) = (i / 100, i % 100);
        sel.push(i);
        keys.push((((col / 30) + row % 3) as usize % 3, -(row as f64) * 0.25));
    }
    // Keys a full sort tells apart by total_cmp (-0 before 0, NaN last) or a point in no file.
    for (k, i) in [(0.0, 5000), (-0.0, 5001), (f64::NAN, 5002), (0.0, 5003), (-0.0, 5004)] {
        sel.push(i);
        keys.push((if i == 5003 { usize::MAX } else { 1 }, k));
    }
    let empty: Vec<(usize, f64)> = Vec::new();
    assert!(super::by_runs(&[], &empty, |a, b| a == b, |a, b| a.0.cmp(&b.0)).is_empty());
    assert_eq!(super::by_runs(&[9], &[(0usize, 0.0)], |a, b| a == b, |a, b| a.0.cmp(&b.0)), [9]);
    let mut full: Vec<(usize, f64, u32)> = keys.iter().zip(&sel).map(|(k, &i)| (k.0, k.1, i)).collect();
    full.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
    let got = super::by_runs(
        &sel,
        &keys,
        |a, b| a.0 == b.0 && a.1.total_cmp(&b.1).is_eq(),
        |a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)),
    );
    assert_eq!(got, full.into_iter().map(|p| p.2).collect::<Vec<_>>());
}

/// A mask's bits answer as its pixels do, in edge blocks too (70 × 50 in 32 × 32 blocks).
#[test]
fn a_mask_answers_as_its_pixels() {
    let (w, h) = (70u32, 50u32);
    let v: Vec<f32> = (0..w * h)
        .map(|p| {
            if (p * 7919) % 13 < 3 && p / w > 33 {
                [3.0, 7.0][(p % 2) as usize]
            } else {
                [0.0, 1.0, 2.9][(p % 3) as usize]
            }
        })
        .collect();
    let gt = [10.0, 0.01, 0.0, 46.0, 0.0, -0.01];
    let tif = crate::geotiff::write_f32(w, h, &v, 32, gt, 4326, None).unwrap();
    let t = Tiff::open(Arc::new(tif.clone())).unwrap();
    let m = Mask::of(Tiff::open(Arc::new(tif)).unwrap()).unwrap();
    // (The top row of blocks holds no void.)
    assert!(m.voids[..3].iter().all(Option::is_none) && m.voids[3..].iter().all(Option::is_some));
    for r in 0..h {
        for c in 0..w {
            let (x, y) = (gt[0] + (c as f64 + 0.5) * gt[1], gt[3] + (r as f64 + 0.5) * gt[5]);
            let want = t.pixel(0, c, r).unwrap() >= super::super::declared::MASK_FILLED;
            assert_eq!(m.masked(x, y), want, "({c}, {r})");
        }
    }
    assert!(!m.masked(9.99, 45.9) && !m.masked(10.71, 45.9) && !m.masked(10.1, 46.01) && !m.masked(10.1, 45.49));
}
