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
    let o = Opts { fetch: &fetch, store: None, pool: None };
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
    let o = Opts { fetch: &f, store: None, pool: None };
    let t = TileSet {
        url: "https://e.org/{z}/{x}/{y}.png",
        zoom: 3,
        encoding: "terrarium",
        o: &o,
        clean: Mutex::new(HashMap::new()),
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
    let o = Opts { fetch: &f, store: None, pool: None };
    let fine_box = [10.0, 50.0 - h as f64 * PX, 10.0 + w as f64 * PX, 50.0];
    let geo = |id: &str, code: u8, res: f64, url: &str, b: [f64; 4]| Dataset {
        id: id.into(),
        code,
        resolution: res,
        cover: None,
        source: Source::Geotiff { crs: 4326, read_resolution: None, store: false, files: vec![(url.into(), Some(b))] },
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
    let o = Opts { fetch: &fetch, store: None, pool: None };
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
    let o = Opts { fetch: &fetch, store: None, pool: None };
    let t = TileSet {
        url: "https://e.org/{z}/{x}/{y}.png",
        zoom: z,
        encoding: "terrarium",
        o: &o,
        clean: Mutex::new(HashMap::new()),
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
