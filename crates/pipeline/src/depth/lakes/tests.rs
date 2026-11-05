use super::*;
use crate::terrain_water::{Kind, Poly};

/// The depth lab's table (validation.md §3, "What the chosen model gives (m)", to its 0.1 m).
#[test]
fn the_labs_table() {
    for (s, l, want) in [
        (0.01, 100.0, 2.3),
        (0.01, 1000.0, 6.6),
        (0.03, 20.0, 0.7),
        (0.03, 200.0, 5.5),
        (0.06, 50.0, 2.4),
        (0.06, 500.0, 10.7),
        (0.12, 10.0, 0.5),
        (0.12, 1000.0, 16.3),
    ] {
        let (d, by_slope) = dmax_estimate(Some(s), l, 1.0);
        assert!(by_slope);
        assert!((d - want).abs() < 0.05, "{s} {l}: {d}");
    }
}

/// L is held at its peak (~1,790 m) beyond it; flat or missing land takes the area rule.
#[test]
fn l_is_held_at_its_peak_and_flat_land_takes_the_area_rule() {
    let peak = (-BETA.2 / (2.0 * BETA.3)).dexp();
    assert!((peak - 1790.0).abs() < 5.0, "{peak}");
    assert_eq!(dmax_estimate(Some(0.05), peak, 1.0), dmax_estimate(Some(0.05), 5000.0, 1.0));
    for s in [None, Some(f64::NAN), Some(0.002)] {
        let (d, by_slope) = dmax_estimate(s, 50.0, 1.0);
        assert!(!by_slope);
        assert!((d - 3.04).abs() < 0.005, "{d}");
    }
    let (d, _) = dmax_estimate(None, 50.0, 5.0);
    assert!((d - 3.04 * 5f64.dpowf(0.228)).abs() < 0.01, "{d}");
}

fn square(lon: f64, lat: f64, d: f64) -> Vec<[f64; 2]> {
    vec![[lon, lat], [lon, lat + d], [lon + d, lat + d], [lon + d, lat], [lon, lat]]
}

/// A pixel is inside where its centre is (even–odd, holes out).
#[test]
fn fill_takes_the_pixels_whose_centre_is_inside() {
    let outer = vec![[1.0, 1.0], [5.0, 1.0], [5.0, 5.0], [1.0, 5.0]];
    let hole = vec![[2.0, 2.0], [3.0, 2.0], [3.0, 3.0], [2.0, 3.0]];
    let m = fill(&[outer.clone()], 0, 0, 6, 6);
    assert_eq!(m.iter().filter(|&&v| v).count(), 16);
    assert!(m[6 + 1] && !m[0] && !m[5 * 6 + 5]);
    let m = fill(&[outer, hole], 0, 0, 6, 6);
    assert_eq!(m.iter().filter(|&&v| v).count(), 15);
    assert!(!m[2 * 6 + 2]);
    // (Offset windows see the same pixels.)
    let m = fill(&[vec![[1.0, 1.0], [5.0, 1.0], [5.0, 5.0], [1.0, 5.0]]], 2, 2, 6, 6);
    assert_eq!(m.iter().filter(|&&v| v).count(), 9);
}

/// Rings as a tile or shapefile holds them: each wound as the first an outer ring, those wound the
/// other way after it its holes; a feature of two parts is both.
#[test]
fn rings_make_parts_and_holes_by_their_winding() {
    use geos::Geom;
    let sq = |x: f64, y: f64, s: f64| vec![[x, y], [x + s, y], [x + s, y + s], [x, y + s]];
    let rev = |mut r: Vec<[f64; 2]>| {
        r.reverse();
        r
    };
    let g = polygon(&[sq(0.0, 0.0, 10.0), rev(sq(2.0, 2.0, 2.0)), sq(20.0, 0.0, 5.0)]).unwrap();
    assert_eq!(g.get_num_geometries().unwrap(), 2);
    assert!((g.area().unwrap() - (100.0 - 4.0 + 25.0)).abs() < 1e-9);
    // (Wound the other way throughout: the same.)
    let g = polygon(&[rev(sq(0.0, 0.0, 10.0)), sq(2.0, 2.0, 2.0), rev(sq(20.0, 0.0, 5.0))]).unwrap();
    assert!((g.area().unwrap() - 121.0).abs() < 1e-9);
}

/// A lake is a bowl to its maximum depth, shallow at the shore, below the surface throughout.
#[test]
fn a_lake_is_a_bowl_to_its_maximum_depth() {
    let rings = to_pixels(&[square(-74.0, 46.0, 0.05)], GLOBATHY_Z);
    let b = Bowl::of(&rings, GLOBATHY_Z).unwrap();
    assert_eq!(b.zoom, GLOBATHY_Z);
    let mut c = Canvas { zoom: GLOBATHY_Z, c0: b.c0, r0: b.r0, w: b.w, h: b.h, v: vec![f32::NAN; b.w * b.h] };
    c.paint(&rings, &b, 20.0);
    let ok: Vec<f32> = c.v.iter().cloned().filter(|v| v.is_finite()).collect();
    assert!(ok.len() > 100);
    let (lo, hi) = ok.iter().fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
    assert!((lo + 20.0).abs() < 1e-3, "{lo}");
    assert!(hi > -2.0, "shallow at the shore: {hi}");
    assert!(ok.iter().all(|&v| v <= 0.0));
}

/// A lake wider than `BOWL_SIDE` pixels is shaped coarser and scaled up: the same bowl, nearly.
#[test]
fn a_wide_lake_is_shaped_coarser_alike() {
    let rings = to_pixels(&[square(-74.0, 46.0, 0.6)], GLOBATHY_Z);
    let b = Bowl::of(&rings, GLOBATHY_Z).unwrap();
    assert!(b.zoom < GLOBATHY_Z && b.w <= BOWL_SIDE + 8, "{} {}", b.zoom, b.w);
    let fine = Bowl::of(&to_pixels(&[square(-74.0, 46.0, 0.6)], b.zoom), b.zoom).unwrap();
    assert_eq!((fine.w, fine.h), (b.w, b.h));
    // (Its middle the deepest, its depth at a quarter across as the fine one's there.)
    let mid = b.at(
        GLOBATHY_Z,
        (b.c0 + b.w as i64 / 2) << (GLOBATHY_Z - b.zoom),
        (b.r0 + b.h as i64 / 2) << (GLOBATHY_Z - b.zoom),
    );
    assert!(mid > 0.98, "{mid}");
}

/// GLOBathy's grids: each z8 tile of the z6 tile with a lake is one grid, cropped to its data, the
/// lakes without a maximum depth left out.
#[test]
fn globathy_paints_its_lakes_into_the_tiles_they_meet() {
    let lakes = vec![
        (1u64, vec![square(-74.0, 46.0, 0.05)]),
        (2, vec![square(-73.8, 46.0, 0.03)]),
        (3, vec![square(-73.6, 46.0, 0.03)]),
    ];
    let dmax: HashMap<u64, f64> = [(1, 20.0), (2, 5.0)].into();
    let [x, y] = to_pixels(&[vec![[-74.0, 46.0]]], 6)[0][0];
    let t = ((x / 256.0) as u32, (y / 256.0) as u32);
    let mut grids = Vec::new();
    globathy_grids(&lakes, &dmax, t, &mut |g| {
        grids.push(g);
        Ok(())
    })
    .unwrap();
    assert!(!grids.is_empty());
    let lo = grids.iter().flat_map(|g| g.v.iter()).cloned().filter(|v| v.is_finite()).fold(f32::MAX, f32::min);
    assert!((lo + 20.0).abs() < 1e-3, "{lo}");
    for g in &grids {
        assert_eq!(g.zoom, GLOBATHY_Z);
        assert!(g.c0 % 256 == 0 && g.r0 % 256 == 0 && g.w % 256 == 0 && g.h % 256 == 0);
        assert!(g.w <= 2048 && g.h <= 2048);
    }
    // (Lake 3 has no depth: its pixels empty.)
    let [x3, y3] = to_pixels(&[vec![[-73.585, 46.015]]], GLOBATHY_Z)[0][0];
    for g in &grids {
        let (i, j) = (x3 as i64 - g.c0, y3 as i64 - g.r0);
        if (0..g.w as i64).contains(&i) && (0..g.h as i64).contains(&j) {
            assert!(g.v[j as usize * g.w + i as usize].is_nan());
        }
    }
}

/// GLOBathy's CSV: its columns by name, quoted fields with commas, the lakes asked for with a
/// maximum depth, wherever their pour point is (the Caspian's is a thousand km from its south).
#[test]
fn globathy_depths_are_read_by_column() {
    let csv = "Hylak_id,Lake_name,Country,Pour_long,Pour_lat,Dmax_use_m\n\
               1,\"Lake A, the big\",Canada,-73.97,46.0,20.0\n\
               2,B,Canada,-73.79,46.0,5.0\n\
               3,C,Canada,-73.59,46.0,\n\
               4,D,Canada,10.0,46.0,7.0\n\
               5,E,Canada,10.0,46.0,9.0\n";
    let d = read_dmax(csv.as_bytes(), &[1, 2, 3, 4].into()).unwrap();
    assert_eq!(d, [(1, 20.0), (2, 5.0), (4, 7.0)].into());
}

/// A pack: its index first, each grid a GeoTIFF on its zoom's grid read back where the index says.
#[test]
fn a_pack_holds_its_grids_where_its_index_says() {
    let mut v = vec![f32::NAN; 512 * 256];
    v[300] = -4.5;
    v[256 * 512 + 7 - 512] = -1.25;
    let g = Canvas { zoom: 14, c0: 512, r0: 1024, w: 512, h: 256, v };
    let p = pack(vec![Tif::of("smalllakes", &g).unwrap()]).unwrap();
    let read = |o: u64, n: usize| -> Result<Vec<u8>> { Ok(p[o as usize..o as usize + n].to_vec()) };
    let idx = unpack_index(&read).unwrap();
    assert_eq!(idx.len(), 1);
    let e = &idx[0];
    assert_eq!((e.zoom, e.c0, e.r0, e.w, e.h), (14, 512, 1024, 512, 256));
    let tiff = &p[e.at[0] as usize..(e.at[0] + e.at[1]) as usize];
    let t = crate::geotiff::Tiff::open(std::sync::Arc::new(tiff.to_vec())).unwrap();
    let gt = t.transform(0).unwrap();
    let px = super::super::grids::px(14);
    assert!((gt[0] - (-super::super::grids::HALF + 512.0 * px)).abs() < 1e-6 && (gt[1] - px).abs() < 1e-9);
    assert_eq!(t.images().len(), 3, "overviews to 64 pixels: 256 and 128 wide");
    assert_eq!(t.pixel(0, 300, 0).unwrap(), -4.5);
    assert_eq!(t.pixel(0, 7, 255).unwrap(), -1.25);
    assert!(t.pixel(0, 0, 0).unwrap().is_nan());
    // (Its tar's own members as a tar reads them.)
    let m = crate::agent::histarchive::list_tar(&p).unwrap();
    assert_eq!(m[0].0, INDEX);
    assert_eq!(m[1].1, tiff);
}

/// The depth reads a pack as the source's grids (grids::resolved): its pack meeting the job's
/// boxes read for its index, each grid sampled from its range; one meeting none left out.
#[test]
fn the_depth_reads_a_pack_as_the_sources_grids() {
    use super::super::grids;
    let d = tempfile::tempdir().unwrap();
    let mut v = vec![f32::NAN; 256 * 256];
    for o in v.iter_mut().skip(256 * 100).take(256) {
        *o = -7.0;
    }
    let (c0, r0) = (256 * 300, 256 * 200);
    let g = Canvas { zoom: 11, c0, r0, w: 256, h: 256, v };
    let p = d.path().join("6-9-6.tar");
    std::fs::write(&p, pack(vec![Tif::of("globathy", &g).unwrap()]).unwrap()).unwrap();
    let t6 = ((c0 >> 13) as u32, (r0 >> 13) as u32);
    let mut s = grids::Source {
        id: "globathy".into(),
        code: 14,
        lake: true,
        band_m: 0.0,
        intertidal: false,
        datum_exact: false,
        rivers: false,
        lakes: false,
        estimated: true,
        label: "G".into(),
        datum: "D".into(),
        notice: None,
        grids: vec![grids::Grid {
            bbox: roadcore::geo::tile_box(6, t6.0, t6.1),
            zoom: 11,
            file: store::nasfs::NasPath::local_for_test(&p),
            low_file: None,
            packed: None,
        }],
        pin: "globathy g".into(),
        made: Some(Method::Globathy),
    };
    let here = grids::window_box(11, c0, r0, 256, 256);
    let r = grids::resolved(std::slice::from_ref(&s), &[here]).unwrap();
    assert_eq!(r[0].grids.len(), 1);
    assert_eq!(r[0].grids[0].bbox, here);
    let got = grids::sample(&grids::Reader::default(), &r[0], 11, c0, r0, 256, 256).unwrap().unwrap();
    assert_eq!(got.v[100 * 256 + 5], -7.0);
    assert!(got.v[0].is_nan());
    // (Read at a coarser zoom from its overviews.)
    let half = grids::sample(&grids::Reader::default(), &r[0], 10, c0 / 2, r0 / 2, 128, 128).unwrap().unwrap();
    assert_eq!(half.v[50 * 128 + 3], -7.0);
    // A box elsewhere: no grid read.
    let far = grids::window_box(11, 0, 0, 16, 16);
    assert!(grids::resolved(std::slice::from_ref(&s), &[far]).unwrap()[0].grids.is_empty());
    s.made = None;
    assert_eq!(grids::resolved(std::slice::from_ref(&s), &[far]).unwrap()[0], s, "a producer's source as it is");
}

/// Logical names: a source's pack of a z6 tile.
#[test]
fn packs_are_named_by_source_and_tile() {
    let l = logical("smalllakes", (40, 21));
    assert_eq!(l, "work/lake-depth/smalllakes/6-40-21");
    assert_eq!(tile_of(&l), Some(("smalllakes", (40, 21))));
    assert_eq!(tile_of("work/lake-depth/smalllakes/7-40-21"), None);
    let m: BTreeMap<String, String> = [(l.clone(), "c".to_string()), ("work/other".into(), "d".into())].into();
    assert_eq!(named(&m).collect::<Vec<_>>(), vec![("smalllakes", (40, 21), "c")]);
}

/// Terrain rising 0.05 m a ground metre eastwards from 100 m at (−73.1°, 45°), or 0 (the sea)
/// where `flat`.
struct Plane {
    flat: bool,
}

impl Terrain for Plane {
    fn tile(&self, x: u32, _y: u32) -> Result<Option<std::sync::Arc<Vec<f32>>>> {
        let r = super::super::grids::px(TERRAIN_Z) * 45f64.to_radians().dcos();
        let base = to_pixels(&[vec![[-73.1, 45.0]]], TERRAIN_Z)[0][0][0];
        let mut v = vec![0f32; 256 * 256];
        if !self.flat {
            for (k, o) in v.iter_mut().enumerate() {
                *o = (100.0 + 0.05 * ((x as usize * 256 + k % 256) as f64 - base) * r) as f32;
            }
        }
        Ok(Some(std::sync::Arc::new(v)))
    }
}

/// A square of `side` ground metres, its south-west corner `east` metres east of (−73°, 45°), as a
/// z14 tile's polygon (in its own pixels) and that tile.
fn pond(kind: Kind, id: u64, east: f64, side: f64) -> ((u32, u32), Poly) {
    let (lon, lat) = (-73.0, 45.0f64);
    let per_lon = roadcore::geo::M_PER_DEG * lat.to_radians().dcos();
    let (w, e) = (lon + east / per_lon, lon + (east + side) / per_lon);
    let n = lat + side / roadcore::geo::M_PER_DEG;
    let r = &to_pixels(&[vec![[w, lat], [e, lat], [e, n], [w, n]]], SMALL_Z)[0];
    let t = ((r[0][0] / 256.0) as u32, (r[3][1] / 256.0) as u32);
    let ring: Vec<[f64; 2]> =
        r.iter().map(|q| [q[0] - f64::from(t.0) * 256.0, q[1] - f64::from(t.1) * 256.0]).collect();
    (t, Poly { kind, id, rings: vec![ring] })
}

/// Only the small lakes HydroLAKES lacks, away from the sea, are drawn: a pond of 2.25 ha on land
/// sloping 5 % gets the lab's depth for its size; one of 16 ha, one under HydroLAKES and one at
/// the sea are left out.
#[test]
fn only_the_small_lakes_hydrolakes_lacks_are_drawn() {
    let ps = [
        pond(Kind::Lake, 1, 0.0, 150.0),
        pond(Kind::Lake, 2, 800.0, 400.0),
        pond(Kind::Lake, 3, 1600.0, 150.0),
        pond(Kind::Lake, 4, 2400.0, 150.0),
        // (A metre from pond 4.)
        pond(Kind::Sea, 5, 2551.0, 150.0),
    ];
    // (Each in its own tile; the z10 tile holding them, and its z14 tiles with a ring.)
    let t10 = (ps[0].0 .0 >> 4, ps[0].0 .1 >> 4);
    let mut water: Vec<((u32, u32), Vec<Poly>)> = Vec::new();
    for y in (t10.1 << 4) - 1..=(t10.1 << 4) + 16 {
        for x in (t10.0 << 4) - 1..=(t10.0 << 4) + 16 {
            water.push(((x, y), ps.iter().filter(|p| p.0 == (x, y)).map(|p| p.1.clone()).collect()));
        }
    }
    let known = {
        let (t, p) = &ps[2];
        let r: Rings = p
            .rings
            .iter()
            .map(|r| r.iter().map(|q| [q[0] + f64::from(t.0) * 256.0, q[1] + f64::from(t.1) * 256.0]).collect())
            .collect();
        polygon(&r).unwrap()
    };
    let mut counts = Counts::new();
    let got = small_lakes(&water, &[known], &Plane { flat: false }, t10, &mut counts).unwrap();
    assert_eq!(got.len(), 1, "{counts:?}");
    assert_eq!(counts, [("by the land slope", 1), ("in HydroLAKES", 1), ("touching the sea", 1)].into());
    // (L ≈ 75 m, S = 0.05: the lab's model.)
    let (want, _) = dmax_estimate(Some(0.05), 75.0, 2.25);
    assert!((got[0].dmax - want).abs() < 0.1 * want, "{} {want}", got[0].dmax);
    let g = small_grid(&got, t10).unwrap();
    let n = g.v.iter().filter(|v| v.is_finite()).count();
    assert!((200..1000).contains(&n), "the pond alone: {n}");
    let lo = g.v.iter().cloned().filter(|v| v.is_finite()).fold(f32::MAX, f32::min);
    assert!((f64::from(lo) + got[0].dmax).abs() < 1e-3);
    // Land at the sea's level isn't land: the area rule.
    let mut counts = Counts::new();
    let flat = small_lakes(&water, &[], &Plane { flat: true }, t10, &mut counts).unwrap();
    assert_eq!(counts.get("by the area rule"), Some(&2), "{counts:?}");
    assert_eq!(flat.len(), 2);
}
