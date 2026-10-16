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
    let tiles = |id: &str, code: u8, url: &str| Dataset { id: id.into(), code, resolution: tile_resolution(z, 0.0), cover: None, source: Source::Tiles { url: url.into(), zoom: z, encoding: "terrarium".into() } };
    let plan = Plan { fmt: 1, version: "t".into(), tag: "1".into(), datasets: vec![tiles("fine", 1, "https://a.org/{z}/{x}/{y}.png"), tiles("coarse", 2, "https://b.org/{z}/{x}/{y}.png")] };
    // The fine dataset has tile 512/511 alone (lon 0 to 0.35°, just north of the equator); the
    // coarse one has it and its neighbour east.
    let mut f = crate::fetch::MapFetch::default();
    f.0.insert("https://a.org/10/512/511.png".into(), Some(flat(100.0)));
    f.0.insert("https://b.org/10/512/511.png".into(), Some(flat(50.0)));
    f.0.insert("https://b.org/10/513/511.png".into(), Some(flat(50.0)));
    let fetch = NoneElse(f);
    let o = Opts { fetch: &fetch, store: None, read_only: true, pool: None };
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
    let o = Opts { fetch: &f, store: None, read_only: true, pool: None };
    let t = TileSet { url: "https://e.org/{z}/{x}/{y}.png", zoom: 3, encoding: "terrarium", o: &o };
    assert_eq!(t.tile(1, 1).unwrap().unwrap().0, 256);
    assert!(t.tile(2, 1).unwrap_err().to_string().contains("512 × 512"));
}
