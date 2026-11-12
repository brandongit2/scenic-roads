use super::mask::{fill_ring_for_test, Shapes};
use super::pyramid::{class, lower, Rows, Tile, Tops};
use super::testkit::{cover_at, height_at, leaf_at, run_sources, sources_file, squares_dir, tiff};
use super::*;

/// The declared canopy source's index (producers/sources/canopy.py's, from the bucket's listing)
/// names, for each file the map reads, a URL the code asked for before the source was declared: the
/// file's own name, or in the equator's row its `lat=-0.0` spelling (asked second). And the
/// leaf-type sources are where the code read them before they were declared, with the boxes it
/// had: the squares are the same.
#[test]
fn the_declared_squares_are_where_the_code_read_them() {
    let s = run_sources();
    let prefix = "https://dataforgood-fb-data.s3.amazonaws.com/forests/v1/alsgedi_global_v6_float_epsg4326_v3_10deg";
    let before = |name: &str| {
        let mut urls = vec![format!("{prefix}/{name}")];
        if let Some(rest) = name.strip_prefix("meta_chm_lat=0.0_") {
            urls.push(format!("{prefix}/meta_chm_lat=-0.0_{rest}"));
        }
        urls
    };
    assert_eq!(s.canopy.len(), 1095, "every kind of every square the bucket has");
    for (name, url) in &s.canopy {
        assert!(before(name).contains(url), "{name}: {url}");
    }
    // (The equator's row: 29 squares, most under `lat=-0.0`.)
    let equator: Vec<&String> =
        s.canopy.iter().filter(|(n, _)| n.starts_with("meta_chm_lat=0.0_")).map(|(_, u)| u).collect();
    assert_eq!(equator.len(), 29 * 3);
    assert!(equator.iter().filter(|u| u.contains("lat=-0.0")).count() > 60);
    assert_eq!(s.canopy[&chm_name(50, 0, "p95")], format!("{prefix}/meta_chm_lat=50.0_lon=0.0_p95.tif"));
    let l = &s.leaf;
    assert_eq!(l.europe, "https://image.discomap.eea.europa.eu/arcgis/rest/services/GioLandPublic/HRL_DominantLeafType2018/ImageServer/exportImage");
    assert_eq!(l.north_america, "https://www.cec.org/files/atlas_layers/1_terrestrial_ecosystems/1_01_0_land_cover_2020_30m/land_cover_2020v2_30m_tif.zip");
    assert_eq!(
        (l.north_america_member, l.europe_box, l.north_america_box),
        ([38952, 2668629085], [-40.0, 20.0, 40.0, 75.0], [-180.0, 14.0, -50.0, 84.0])
    );
}

/// The coverage: most of block 8/132/88's west, a hole in it, and another shape over the hole's
/// corner.
const COVERAGE: &str = r#"{"shapes": [[[[5.5, 47.5], [5.7, 47.5], [5.7, 49.5], [5.5, 49.5]], [[5.64, 48.9], [5.66, 48.9], [5.66, 48.91], [5.64, 48.91], [5.64, 48.9]]], [[[5.655, 48.905], [5.68, 48.905], [5.67, 48.93]]]]}"#;

fn decode(webp: &[u8]) -> Vec<u8> {
    let mut d = image_webp::WebPDecoder::new(std::io::Cursor::new(webp)).unwrap();
    let mut out = vec![0u8; d.output_buffer_size().unwrap()];
    d.read_image(&mut out).unwrap();
    if d.has_alpha() {
        out.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect()
    } else {
        out
    }
}

#[test]
fn geometry_as_trees_py() {
    // trees.py's tile_bounds(8, 132, 88): its longitudes exactly, its latitudes (Apple's maths
    // there) to an ulp or two.
    let b = roadcore::geo::tile_box(8, 132, 88);
    assert_eq!((b[0], b[2]), (5.625, 7.03125));
    for (got, want) in [(b[1], 47.98992166741417), (b[3], 48.92249926375824)] {
        assert!((got - want).abs() <= 2.0 * f64::EPSILON * want, "{got} vs {want}");
    }
    assert_eq!(roadcore::geo::tile_box(8, 0, 0)[0], -180.0);
    assert!((roadcore::geo::tile_box(8, 0, 0)[3] - 85.0511287798066).abs() < 1e-13);
    // The squares a block meets: one, or (across 50° N) two, top first.
    assert_eq!(squares_of(b), [(50, 0)]);
    assert_eq!(squares_of(roadcore::geo::tile_box(8, 130, 86)), [(60, 0), (50, 0)]);
    assert_eq!(squares_of([-0.5, 9.5, 0.5, 10.5]), [(20, -10), (20, 0), (10, -10), (10, 0)]);
    assert_eq!(
        (chm_name(50, -10, "p95"), leaf_name(50, -10)),
        ("meta_chm_lat=50.0_lon=-10.0_p95.tif".to_string(), "lat50_lon-10.tif".to_string())
    );
    // A square's pixel for a latitude and longitude, none outside it.
    let p = indices(50, 0, CHM_RES, &[-0.0001, 0.0, 9.99999, 10.0], &[49.9999, 40.0001, 40.0, 50.0]);
    assert_eq!(p.n, 40000);
    assert_eq!(p.rows, [Some(0), Some(39999), None, Some(0)]);
    assert_eq!(p.cols, [None, Some(0), Some(39999), None]);
    // trees.py's Web Mercator metres, to the last few bits (roadcore::geo's projection).
    assert!((roadcore::geo::epsg3857(5.625, 48.92)[0] - 626172.1357121639).abs() < 1e-8);
    assert!((roadcore::geo::epsg3857(5.625, 48.92)[1] - 6261297.953407845).abs() < 1e-8);
}

#[test]
fn rings_densified_and_burnt_as_gdal() {
    // trees.py's densify of [[0, 0], [0.12, 0], [0.12, 0.07], [0, 0]].
    let s =
        Shapes::parse(r#"{"shapes": [[[[0, 0], [0.12, 0], [0.12, 0.07], [0, 0]], [[1, 1], [2, 2]]], []]}"#).unwrap();
    assert_eq!((s.shapes.len(), s.shapes[0].len(), s.shapes[1].len()), (2, 1, 0), "a ring of two points left out");
    assert_eq!(s.shapes[0][0].bbox, [0.0, 0.0, 0.12, 0.07]);
    let lens = [
        (0.039999999999999994, 0.0),
        (0.07999999999999999, 0.0),
        (0.12, 0.035),
        (0.08, 0.046666666666666676),
        (0.04000000000000001, 0.023333333333333338),
    ];
    let r = s.shapes[0][0].merc_for_test();
    for (lon, lat) in lens {
        assert!(r.contains(&roadcore::geo::epsg3857(lon, lat)), "{lon} {lat}");
    }
    assert_eq!(r.len(), 9);
    // A rectangle in pixels: the pixels whose centres it holds.
    let bits = fill_ring_for_test(&[[10.2, 10.2], [20.7, 10.2], [20.7, 20.7], [10.2, 20.7], [10.2, 10.2]]);
    let on = |x: usize, y: usize| bits[(y * BS + x) / 64] >> ((y * BS + x) % 64) & 1 == 1;
    assert!(on(10, 10) && on(20, 20) && !on(9, 10) && !on(21, 20) && !on(10, 21));
    assert_eq!(bits.iter().map(|w| w.count_ones()).sum::<u32>(), 11 * 11);
    // Off the block: nothing.
    assert!(fill_ring_for_test(&[[-5.0, -5.0], [-1.0, -5.0], [-1.0, 5000.0], [-5.0, 5000.0], [-5.0, -5.0]])
        .iter()
        .all(|&w| w == 0));
}

#[test]
fn shapes_holes_and_unions() {
    let s = Shapes::parse(COVERAGE).unwrap();
    let b = roadcore::geo::tile_box(8, 132, 88);
    assert!(s.meets(b) && !s.meets(roadcore::geo::tile_box(8, 140, 88)));
    let m = mask::inside(&s.meeting(b), b);
    let px = |lon: f64, lat: f64| {
        let ([x0, y0], [x1, y1], [x, y]) = (
            roadcore::geo::epsg3857(b[0], b[3]),
            roadcore::geo::epsg3857(b[2], b[1]),
            roadcore::geo::epsg3857(lon, lat),
        );
        let (i, j) = (((y - y0) / (y1 - y0) * BS as f64) as usize, ((x - x0) / (x1 - x0) * BS as f64) as usize);
        m[(i * BS + j) / 64] >> ((i * BS + j) % 64) & 1 == 1
    };
    assert!(px(5.63, 48.5) && px(5.69, 48.0), "inside the first ring");
    assert!(!px(5.72, 48.5), "east of it");
    assert!(!px(5.645, 48.902), "in its hole");
    assert!(px(5.659, 48.909), "in the hole, but inside the other shape");
}

#[test]
fn classes_means_and_their_order() {
    assert_eq!(class([0, 0, 0, 0]), 255);
    assert_eq!(class([3, 1, 1, 1]), 1, "forest half of it, the first of a tie");
    assert_eq!(class([4, 1, 1, 1]), 0);
    assert_eq!(class([0, 1, 5, 5]), 2);
    // numpy's two-axis mean: (a + b) + (c + d), in float32.
    let r = Rows {
        width: 2,
        cover: vec![16777216.0, 1.0, 1.0, 1.0],
        height: vec![0.0; 4],
        leaf: [vec![1, 0, 0, 0], vec![0; 4], vec![0; 4], vec![0, 1, 1, 1], vec![0; 4]],
    };
    let d = r.down_for_test();
    assert_eq!(d.cover, [4194304.5]);
    assert_eq!(d.leaf[0], [1]);
    assert_eq!(d.leaf[3], [3]);
    // A block's zoom-8 values, written and read back.
    let mut t = Tops {
        x: 3,
        y: 9,
        rows: Rows {
            width: TS,
            cover: (0..TS * TS).map(|i| i as f32 * 0.37).collect(),
            height: vec![1.5; TS * TS],
            leaf: Default::default(),
        },
    };
    for (k, l) in t.rows.leaf.iter_mut().enumerate() {
        *l = (0..TS * TS).map(|i| ((i * (k + 1)) % 257) as u32).collect();
    }
    let b = t.to_bytes().unwrap();
    assert_eq!(Tops::block_of(&b).unwrap(), (3, 9));
    assert_eq!(Tops::from_bytes(&b).unwrap(), t);
    assert!(Tops::from_bytes(&b[..20]).is_err());
}

#[test]
fn zooms_above_the_blocks() {
    // Two blocks of z7 tile 7/66/44: one all cover 40 % and height 10 m, broadleaf; the other
    // nothing; the other two quarters no block.
    let n = TS * TS;
    let full = Tops {
        x: 132,
        y: 88,
        rows: Rows {
            width: TS,
            cover: vec![40.0; n],
            height: vec![10.0; n],
            leaf: [vec![0; n], vec![256; n], vec![0; n], vec![0; n], vec![0; n]],
        },
    };
    let none = Tops {
        x: 133,
        y: 89,
        rows: Rows {
            width: TS,
            cover: vec![0.0; n],
            height: vec![0.0; n],
            leaf: [vec![256; n], vec![0; n], vec![0; n], vec![0; n], vec![0; n]],
        },
    };
    let tops: BTreeMap<(u32, u32), Vec<u8>> =
        [full, none].iter().map(|t| ((t.x, t.y), t.to_bytes().unwrap())).collect();
    let said = std::sync::Mutex::new(Vec::new());
    let tiles = lower(&tops, &|d, t| said.lock().unwrap().push((d, t))).unwrap();
    let said = said.into_inner().unwrap();
    assert_eq!((said.first(), said.last()), (Some(&(0, 4)), Some(&(4, 4))), "one tile at each of zoom 7 to 4");
    let keys: Vec<(u8, u8, u32, u32)> = tiles.iter().map(|t| (t.layer, t.z, t.x, t.y)).collect();
    assert_eq!(
        keys,
        [
            (0, 7, 66, 44),
            (1, 7, 66, 44),
            (2, 7, 66, 44),
            (0, 6, 33, 22),
            (1, 6, 33, 22),
            (2, 6, 33, 22),
            (0, 5, 16, 11),
            (1, 5, 16, 11),
            (2, 5, 16, 11),
            (0, 4, 8, 5),
            (1, 4, 8, 5),
            (2, 4, 8, 5)
        ]
    );
    // Zoom 7: the full block's quarter 40 % (its pixels' means) and broadleaf, the rest nothing.
    let rgb = |t: &Tile, x: usize, y: usize| {
        let d = decode(&t.webp);
        let i = (y * TS + x) * 3;
        (d[i] as u32) << 8 | d[i + 1] as u32
    };
    assert_eq!((rgb(&tiles[0], 10, 10), rgb(&tiles[0], 200, 10), rgb(&tiles[0], 10, 200)), (32768 + 40, 32768, 32768));
    assert_eq!(
        (rgb(&tiles[2], 10, 10), rgb(&tiles[2], 200, 200)),
        (32768 + 1, 32768),
        "broadleaf; not forest (no data shows as 0)"
    );
    // Zoom 4: the block is 16 × 16 of its pixels (64..80, 128..144).
    assert_eq!(
        (rgb(&tiles[9], 70, 130), rgb(&tiles[9], 0, 0), rgb(&tiles[11], 70, 130)),
        (32768 + 40, 32768, 32768 + 1)
    );
}

/// A block's zoom-12 pixel (i, j) as trees.py makes it from the synthetic squares: cover, height
/// and leaf type, Terrarium-encoded; `inside` whether the coverage holds it.
fn expected(bx: u32, by: u32, i: usize, j: usize, inside: bool) -> [u32; 3] {
    let lon = lon_of((j + bx as usize * BS) as f64 + 0.5, ZMAX);
    let lat = lat_of((i + by as usize * BS) as f64 + 0.5, ZMAX);
    let (r, c) = (((50.0 - lat) / CHM_RES).floor() as u32, (lon / CHM_RES).floor() as u32);
    let data = (43 * 512..46 * 512).contains(&c) && (8 * 512..9 * 512).contains(&r);
    let (cv, hv) = if data { (cover_at(r, c), height_at(r, c)) } else { (0, 0) };
    let (lr, lc) = (((50.0 - lat) / LEAF_RES).floor() as u32, (lon / LEAF_RES).floor() as u32);
    let lv = if (21 * 512..23 * 512).contains(&lc) && (4 * 512..5 * 512).contains(&lr) { leaf_at(lr, lc) } else { 255 };
    let cov = if inside && cv <= 1000 { (cv as f64 / 10.0) as f32 } else { 0.0 };
    let hgt = if inside && cov >= 5.0 { (hv as f64 / 100.0) as f32 } else { 0.0 };
    let l = if inside { lv } else { 255 };
    let l = if (1..=3).contains(&l) && cov < 5.0 { 0 } else { l };
    let enc = |v: f32, step: f32| ((v / step).round_ties_even() * step) as u32 + 32768;
    [enc(cov, 2.0), enc(hgt, 2.0), enc(if l == 255 { 0.0 } else { l as f32 }, 1.0)]
}

#[test]
fn a_block_from_its_squares() {
    let d = squares_dir();
    let shapes = Shapes::parse(COVERAGE).unwrap();
    let fetch = buildstore::fetch::MapFetch::default();
    let inp = Inputs {
        chm: Source::Dir(d.path().into()),
        leaf: Source::Dir(d.path().into()),
        fetch: &fetch,
        record: None,
        there: None,
    };
    let out = block(&shapes, &inp, 132, 88).unwrap();
    // Tiles only where there's something: zoom 12's first two down the west (the squares' data
    // reach the second; the coverage ends within the first column) and those above them.
    let z12: Vec<(u8, u32, u32)> = out.tiles.iter().filter(|t| t.z == 12).map(|t| (t.layer, t.x, t.y)).collect();
    assert_eq!(
        z12,
        [(0, 2112, 1408), (1, 2112, 1408), (2, 2112, 1408), (0, 2112, 1409), (1, 2112, 1409), (2, 2112, 1409)]
    );
    assert_eq!(out.tiles.len(), 18);
    // Pixel by pixel as trees.py has them (the hole of the coverage and its edge among them).
    let b = roadcore::geo::tile_box(8, 132, 88);
    let m = mask::inside(&shapes.meeting(b), b);
    for t in out.tiles.iter().filter(|t| t.z == 12) {
        let px = decode(&t.webp);
        for i in (0..TS).step_by(3) {
            for j in (0..TS).step_by(5) {
                let (bi, bj) = ((t.y as usize - 1408) * TS + i, (t.x as usize - 2112) * TS + j);
                let inside = m[(bi * BS + bj) / 64] >> ((bi * BS + bj) % 64) & 1 == 1;
                let k = (i * TS + j) * 3;
                let got = (px[k] as u32) << 8 | px[k + 1] as u32;
                assert_eq!(
                    got,
                    expected(132, 88, bi, bj, inside)[t.layer as usize],
                    "tile {:?} pixel {i},{j}",
                    (t.layer, t.x, t.y)
                );
            }
        }
    }
    // As files, and assembled: the same tiles, then zoom 7 to 4.
    let o = tempfile::tempdir().unwrap();
    block_files(&shapes, &inp, 132, 88, &o.path().join("b")).unwrap();
    let n = assemble(&[o.path().join("b")], &o.path().join("z3"), &|_, _| {}).unwrap();
    assert_eq!(n, [10, 10, 10], "zoom 12's two, one at each of zoom 11 to 4");
}

#[test]
fn the_z3_run_and_its_blocks_alike() {
    let d = squares_dir();
    let o = tempfile::tempdir().unwrap();
    // Inside blocks 8/132/88 and 8/133/88.
    const COV: &str = r#"{"shapes": [[[[5.66, 48.2], [7.2, 48.2], [7.2, 48.91], [5.66, 48.91]]]]}"#;
    std::fs::write(o.path().join("cov.json"), COV).unwrap();
    let run = |threads: usize, out: &str| {
        let a = Run {
            tile: (3, 4, 2),
            coverage: o.path().join("cov.json"),
            chm: d.path().into(),
            chm_store: o.path().join("store"),
            sources: sources_file(o.path()),
            leaf: d.path().into(),
            out: o.path().join(out),
            keep: None,
        };
        rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap().install(|| z3(&a)).unwrap()
    };
    let n1 = run(1, "one");
    let n4 = run(4, "four");
    assert_eq!(n1, n4);
    // The blocks its rings' boxes meet, assembled in any order: the same archives.
    let shapes = Shapes::parse(COV).unwrap();
    let blocks = z3_blocks(&shapes, 4, 2);
    assert_eq!(blocks, [(132, 88), (133, 88)]);
    let fetch = buildstore::fetch::MapFetch::default();
    let inp = Inputs {
        chm: Source::Dir(d.path().into()),
        leaf: Source::Dir(d.path().into()),
        fetch: &fetch,
        record: None,
        there: None,
    };
    let dirs: Vec<PathBuf> = blocks
        .iter()
        .rev()
        .map(|&(x, y)| {
            let p = o.path().join(format!("b-{x}-{y}"));
            block_files(&shapes, &inp, x, y, &p).unwrap();
            p
        })
        .collect();
    assert_eq!(assemble(&dirs, &o.path().join("asm"), &|_, _| {}).unwrap(), n1);
    for l in LAYERS {
        let f = format!("trees-{l}.tiles");
        let one = std::fs::read(o.path().join("one").join(&f)).unwrap();
        assert_eq!(one, std::fs::read(o.path().join("four").join(&f)).unwrap(), "{l}: 1 thread and 4 alike");
        assert_eq!(one, std::fs::read(o.path().join("asm").join(&f)).unwrap(), "{l}: the blocks assembled alike");
    }
}

/// An archive's tiles: (z, x, y) → bytes.
fn tiles_of(p: &Path) -> BTreeMap<(u8, u32, u32), Vec<u8>> {
    let a = Archive::open(p).unwrap();
    a.entries()
        .iter()
        .map(|e| {
            (
                ((e.key >> 58) as u8, ((e.key >> 29) & ((1 << 29) - 1)) as u32, (e.key & ((1 << 29) - 1)) as u32),
                a.get_entry(e).to_vec(),
            )
        })
        .collect()
}

#[test]
fn a_z3_tiles_pieces_and_their_assembly_make_its_tiles() {
    let d = squares_dir();
    let o = tempfile::tempdir().unwrap();
    // Over blocks 8/131–134, rows 87 and 88: z6 tiles 6/32/21, 6/32/22, 6/33/21 and 6/33/22 of z3
    // tile 3/4/2, the squares' data in each.
    const COV: &str = r#"{"shapes": [[[[5.5, 48.85], [9.5, 48.85], [9.5, 48.95], [5.5, 48.95]]]]}"#;
    std::fs::write(o.path().join("cov.json"), COV).unwrap();
    let run = |tile: (u8, u32, u32), out: &str| Run {
        tile,
        coverage: o.path().join("cov.json"),
        chm: d.path().into(),
        chm_store: o.path().join("store"),
        sources: sources_file(o.path()),
        leaf: d.path().into(),
        out: o.path().join(out),
        keep: None,
    };
    let pool = |n: usize| rayon::ThreadPoolBuilder::new().num_threads(n).build().unwrap();
    let n3 = pool(4).install(|| z3(&run((3, 4, 2), "z3"))).unwrap();
    let shapes = Shapes::parse(COV).unwrap();
    let mut pieces: Vec<(u32, u32)> = z3_blocks(&shapes, 4, 2).iter().map(|&(x, y)| (x >> 2, y >> 2)).collect();
    pieces.sort_unstable();
    pieces.dedup();
    assert_eq!(pieces, [(32, 21), (32, 22), (33, 21), (33, 22)]);
    assert_eq!(
        blocks_of(&shapes, 6, 33, 22),
        [(132, 88), (133, 88), (134, 88)],
        "a z6 tile's blocks, as the z3 tile's"
    );
    let mut mids = Vec::new();
    for &(x, y) in &pieces {
        let out = format!("z6-{x}-{y}");
        pool(4).install(|| z6(&run((6, x, y), &out))).unwrap();
        mids.push(o.path().join(&out).join(MID));
    }
    let n = assemble_lo(&mids, &o.path().join("lo"), &|_, _| {}).unwrap();
    for (i, l) in LAYERS.iter().enumerate() {
        let f = format!("trees-{l}.tiles");
        let whole = tiles_of(&o.path().join("z3").join(&f));
        assert_eq!(whole.len(), n3[i]);
        let mut parts = tiles_of(&o.path().join("lo").join(&f));
        assert_eq!(parts.len(), n[i]);
        assert!(parts.keys().all(|k| (4..=8).contains(&k.0)), "{l}: the assembly's zoom 8 to 4");
        for &(x, y) in &pieces {
            let p = tiles_of(&o.path().join(format!("z6-{x}-{y}")).join(&f));
            assert!(!p.is_empty() || i == 2, "{l}: tiles in 6/{x}/{y}");
            assert!(
                p.keys().all(|k| (9..=12).contains(&k.0) && (k.1 >> (k.0 - 6), k.2 >> (k.0 - 6)) == (x, y)),
                "{l}: 6/{x}/{y}'s own zoom 9 to 12"
            );
            parts.extend(p);
        }
        assert_eq!(parts, whole, "{l}: the pieces and their assembly, tile for tile");
    }
    // A mid read back: its tile, every block's values, its blocks' zoom-8 tiles.
    let m = read_mid(&mids[3]).unwrap();
    assert_eq!((m.tile, m.tops.keys().copied().collect::<Vec<_>>()), ((33, 22), vec![(132, 88), (133, 88), (134, 88)]));
    assert!(m.z8.iter().all(|t| t.z == 8) && m.z8.iter().any(|t| (t.x, t.y) == (132, 88)));
    // The same on one thread and on three: the same mid, byte for byte, and the same tiles.
    for n in [1, 3] {
        let again = format!("again-{n}");
        pool(n).install(|| z6(&run((6, 33, 22), &again))).unwrap();
        assert_eq!(std::fs::read(o.path().join(&again).join(MID)).unwrap(), std::fs::read(&mids[3]).unwrap());
        for l in LAYERS {
            let f = format!("trees-{l}.tiles");
            assert_eq!(tiles_of(&o.path().join(&again).join(&f)), tiles_of(&o.path().join("z6-33-22").join(&f)));
        }
    }
    // Assembled again in another order: the same archives.
    let rev: Vec<PathBuf> = mids.iter().rev().cloned().collect();
    assemble_lo(&rev, &o.path().join("lo2"), &|_, _| {}).unwrap();
    for l in LAYERS {
        let f = format!("trees-{l}.tiles");
        assert_eq!(
            std::fs::read(o.path().join("lo2").join(&f)).unwrap(),
            std::fs::read(o.path().join("lo").join(&f)).unwrap()
        );
    }
    // Not a tile twice, nor another z3 tile's.
    assert!(assemble_lo(&[mids[0].clone(), mids[0].clone()], &o.path().join("x"), &|_, _| {}).is_err());
    let far = o.path().join("far.sect");
    write_mid(&far, (40, 22), &BTreeMap::new()).unwrap();
    assert_eq!(read_mid(&far).unwrap(), Mid { tile: (40, 22), z8: Vec::new(), tops: BTreeMap::new() });
    assert!(assemble_lo(&[mids[0].clone(), far], &o.path().join("x"), &|_, _| {}).is_err());
    // A block's values in another tile's mid: refused as written.
    let tops = m.tops[&(132, 88)].clone();
    assert!(write_mid(&o.path().join("bad.sect"), (32, 22), &[((132, 88), (Vec::new(), tops))].into()).is_err());
}

/// Task #889: a z6 tile's run paused at every one of its safe points (each row of blocks kept),
/// resumed each time, writes the same mid and archives as one never paused.
#[test]
fn a_z6_run_paused_at_every_safe_point_resumes_to_the_same_bytes() {
    use jobrt::resume_log::STOP_AFTER;
    let d = squares_dir();
    let o = tempfile::tempdir().unwrap();
    const COV: &str = r#"{"shapes": [[[[8.5, 48.6], [9.5, 48.6], [9.5, 48.95], [8.5, 48.95]]]]}"#;
    std::fs::write(o.path().join("cov.json"), COV).unwrap();
    let run = |out: &str, keep: Option<PathBuf>| Run {
        tile: (6, 33, 22),
        coverage: o.path().join("cov.json"),
        chm: d.path().into(),
        chm_store: o.path().join("store"),
        sources: sources_file(o.path()),
        leaf: d.path().into(),
        out: o.path().join(out),
        keep,
    };
    let files = |out: &str| {
        std::iter::once(MID.to_string())
            .chain(LAYERS.iter().map(|l| format!("trees-{l}.tiles")))
            .map(|f| std::fs::read(o.path().join(out).join(f)).unwrap())
            .collect::<Vec<_>>()
    };
    z6(&run("fresh", None)).unwrap();
    let keep = o.path().join("keep");
    let mut stops = 0;
    loop {
        STOP_AFTER.set(Some(1));
        let r = z6(&run("paused", Some(keep.clone())));
        STOP_AFTER.set(None);
        match r {
            Err(e) => {
                assert!(jobrt::control::is_paused(&e), "{e:#}");
                stops += 1;
            }
            Ok(_) => break,
        }
    }
    assert!(stops >= 1, "a row a safe point: {stops}");
    assert!(files("paused") == files("fresh"), "paused at each of its {stops} safe points");
}

#[test]
fn results_in_order_on_any_threads() {
    for threads in [1, 3, 8] {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
        let mut got = Vec::new();
        pool.install(|| {
            in_order(
                50,
                |i| {
                    std::thread::sleep(std::time::Duration::from_micros(((i * 7919) % 13) as u64 * 100));
                    Ok(i * i)
                },
                |_| {},
                |i, v| {
                    got.push((i, v));
                    Ok(())
                },
            )
        })
        .unwrap();
        assert_eq!(got, (0..50).map(|i| (i, i * i)).collect::<Vec<_>>(), "{threads} threads");
        let failed = pool.install(|| {
            in_order(50, |i| if i == 17 { anyhow::bail!("block {i}") } else { Ok(i) }, |_| {}, |_, _| Ok(()))
        });
        assert!(failed.is_err_and(|e| e.to_string() == "block 17"));
        if threads > 1 {
            // A block that panics on a thread fails the run, rather than leave it waiting; as does
            // a panic where the results are written.
            let panicked = pool
                .install(|| in_order(50, |i| if i == 23 { panic!("block {i}") } else { Ok(i) }, |_| {}, |_, _| Ok(())));
            assert!(panicked.is_err_and(|e| e.to_string() == "item 23 panicked: block 23"));
            let sunk = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                pool.install(|| in_order(50, Ok, |_| {}, |i, _| if i == 5 { panic!("written") } else { Ok(()) }))
            }));
            assert!(sunk.is_err());
        }
    }
}

#[test]
fn items_with_parallel_work_of_their_own_never_hang_it() {
    // (The pool's threads once waited in `in_order` for room, and an item's parallel work waiting on
    // such a thread never ended: many items, more than its window, each with parallel work (the
    // first slow), and a pool thread busy as it starts, as z3's blocks. The items' parallel work
    // runs in the test's own pool, the one `in_order` is called from: never queued behind the other
    // tests' work in rayon's global pool, which can hold it for minutes. A hang is told from a slow
    // run by its work: none of it moves for two minutes, however slowly a busy Mac runs it.)
    use rayon::prelude::*;
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    const RUNS: u64 = 3;
    const ITEMS: u64 = 300;
    const STEPS: u64 = 64;
    const STALL: std::time::Duration = std::time::Duration::from_secs(120);
    let steps = std::sync::Arc::new(AtomicU64::new(0));
    let (tx, rx) = std::sync::mpsc::channel();
    let done = steps.clone();
    std::thread::spawn(move || {
        let pool = std::sync::Arc::new(rayon::ThreadPoolBuilder::new().num_threads(14).build().unwrap());
        for _ in 0..RUNS {
            let got = pool.install(|| {
                rayon::spawn(|| std::thread::sleep(std::time::Duration::from_millis(100)));
                let mut sum = 0u64;
                in_order(
                    ITEMS as usize,
                    |i| {
                        let pause = std::time::Duration::from_micros(if i == 0 { 3000 } else { 150 });
                        Ok(pool.install(|| {
                            (0..STEPS)
                                .into_par_iter()
                                .map(|x| {
                                    std::thread::sleep(pause);
                                    done.fetch_add(1, Relaxed);
                                    x * i as u64
                                })
                                .sum::<u64>()
                        }))
                    },
                    |_| {},
                    |_, v| {
                        sum += v;
                        Ok(())
                    },
                )
                .map(|_| sum)
            });
            tx.send(got.map_err(|e| e.to_string())).ok();
        }
    });
    for run in 0..RUNS {
        let mut seen = steps.load(Relaxed);
        let got = loop {
            match rx.recv_timeout(STALL) {
                Ok(got) => break got,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    let now = steps.load(Relaxed);
                    assert!(
                        now > seen,
                        "in_order hung in run {run}: no item's work moved in {STALL:?} ({now} of {} steps done)",
                        RUNS * ITEMS * STEPS
                    );
                    seen = now;
                }
                Err(e) => panic!("in_order's thread ended without its result: {e}"),
            }
        };
        assert_eq!(got.unwrap(), (0..ITEMS).map(|i| (0..STEPS).map(|x| x * i).sum::<u64>()).sum::<u64>());
    }
}

#[test]
fn reads_recorded_and_read_back() {
    // A block run on its squares, recording what it reads; then run again from just that, through
    // the fetch layer's mirror: the same tiles.
    let d = squares_dir();
    let shapes = Shapes::parse(COVERAGE).unwrap();
    let m = tempfile::tempdir().unwrap();
    let none = buildstore::fetch::MapFetch::default();
    let inp = Inputs {
        chm: Source::Dir(d.path().into()),
        leaf: Source::Dir(d.path().into()),
        fetch: &none,
        record: Some(m.path().into()),
        there: None,
    };
    let a = block(&shapes, &inp, 132, 88).unwrap();
    assert!(a.kept > 0 && a.kept < 8 << 20, "{} bytes kept", a.kept);
    let fetch = buildstore::fetch::Fetcher::new(Some(m.path().into()), false);
    let url = format!("file://{}", std::path::absolute(d.path()).unwrap().display());
    let inp = Inputs { chm: Source::parse(&url), leaf: Source::parse(&url), fetch: &fetch, record: None, there: None };
    let b = block(&shapes, &inp, 132, 88).unwrap();
    assert_eq!(a.tiles, b.tiles);
    assert_eq!(a.tops, b.tops);
}

#[test]
fn leaf_type_squares_complete() {
    let d = tempfile::tempdir().unwrap();
    let small = |meta: &[(&str, &str)]| tiff(1024, 8, &|_, _| None, meta);
    std::fs::write(d.path().join("a.tif"), small(&[("complete", "1")])).unwrap();
    // A NALCMS square resampled with the Mac's libm (its old tag) is made again; one with det's is kept.
    std::fs::write(
        d.path().join("b.tif"),
        small(&[("complete", "1"), ("source", "NALCMS 2020 land cover 30 m (CEC), resampled to 0.0005°")]),
    )
    .unwrap();
    std::fs::write(d.path().join("f.tif"), small(&[("complete", "1"), ("source", crate::leaftype::NALCMS_SOURCE)]))
        .unwrap();
    std::fs::write(d.path().join("c.tif"), small(&[("source", "Copernicus HRL Dominant Leaf Type 2018 (EEA)")]))
        .unwrap();
    let mut cut = small(&[("complete", "1")]);
    cut.truncate(100);
    std::fs::write(d.path().join("d.tif"), cut).unwrap();
    let c = |f: &str| squares::complete(&d.path().join(f));
    assert_eq!(
        (c("a.tif"), c("b.tif"), c("c.tif"), c("d.tif"), c("e.tif"), c("f.tif")),
        (true, false, false, false, false, true)
    );
    // None to make (outside both sources' boxes, or complete): nothing is asked.
    std::fs::write(d.path().join(leaf_name(50, 0)), small(&[("complete", "1")])).unwrap();
    let s = run_sources();
    squares::leaf_types(&[(50, 0), (40, 120)], d.path(), &s.leaf).unwrap();
}

#[test]
fn canopy_squares_from_the_store_or_this_macs_copies() {
    use squares::Here;
    let d = tempfile::tempdir().unwrap();
    let (chm, store) = (d.path().join("chm"), d.path().join("store"));
    std::fs::create_dir_all(&chm).unwrap();
    std::fs::create_dir_all(&store).unwrap();
    let t = tiff(1024, 16, &|_, _| None, &[]);
    for k in ["cover5m", "p95"] {
        std::fs::write(store.join(chm_name(50, 0, k)), &t).unwrap();
        std::fs::write(store.join(chm_name(40, 0, k)), b"").unwrap();
    }
    let said = std::sync::Mutex::new(Vec::new());
    let got = squares::canopy(&chm, &store, 50, 0, &|f| said.lock().unwrap().push(f), &BTreeMap::new()).unwrap();
    assert_eq!(said.into_inner().unwrap(), [0.5, 1.0]);
    // Read where they lie: nothing copied.
    assert_eq!(
        got,
        Some([Here::Nas(store.join(chm_name(50, 0, "cover5m"))), Here::Nas(store.join(chm_name(50, 0, "p95")))])
    );
    assert!(!chm.join(chm_name(50, 0, "p95")).exists());
    // Meta's "none": empty files, the square not there.
    assert_eq!(squares::canopy(&chm, &store, 40, 0, &|_| {}, &BTreeMap::new()).unwrap(), None);
    // This Mac's whole copy read here; one cut short passed over for the store's.
    std::fs::write(chm.join(chm_name(50, 0, "cover5m")), &t).unwrap();
    std::fs::write(chm.join(chm_name(50, 0, "p95")), &t[..50]).unwrap();
    let got = squares::canopy(&chm, &store, 50, 0, &|_| {}, &BTreeMap::new()).unwrap();
    assert_eq!(
        got,
        Some([Here::Local(chm.join(chm_name(50, 0, "cover5m"))), Here::Nas(store.join(chm_name(50, 0, "p95")))])
    );
    store::cachefile::release(&chm.join(chm_name(50, 0, "cover5m")));
}

/// The rows a run reads of a canopy file on the NAS, read first into a local file (task #606): every
/// read the same bytes as the file's, the strips holding the rows from the local file.
#[test]
fn canopy_rows_read_first_are_the_files_bytes() {
    let d = squares_dir();
    let o = tempfile::tempdir().unwrap();
    let p = d.path().join(chm_name(50, 0, "cover5m"));
    let whole = std::fs::read(&p).unwrap();
    let n = std::sync::atomic::AtomicU64::new(0);
    let (f, read) = squares::rows_here(&p, (4100, 4500), &o.path().join("rows"), None, &|done, _| {
        n.store(done, std::sync::atomic::Ordering::Relaxed)
    })
    .unwrap();
    let kept = std::fs::metadata(o.path().join("rows")).unwrap().len();
    assert!(kept > 0 && kept < whole.len() as u64 && n.into_inner() == kept && read == kept);
    assert_eq!(f.len().unwrap(), whole.len() as u64);
    for (off, len) in [(0, 16), (1000, 4096), (whole.len() as u64 / 2, 9000), (whole.len() as u64 - 100, 100)] {
        assert_eq!(f.read_at(off, len).unwrap(), whole[off as usize..off as usize + len]);
    }
    // At the square's edges: its first row, its last, and rows past its end (its last strip); a
    // sparse strip (none stored) read where it lies.
    let img = buildstore::geotiff::Tiff::open(std::sync::Arc::new(store::range::PlainFile::open(&p).unwrap()))
        .unwrap()
        .level(0)
        .unwrap()
        .height;
    for (k, rows) in [(0, 0), (img - 1, img - 1), (img - 3, img + 500), (img + 10, img + 20)].into_iter().enumerate() {
        let (f, _) = squares::rows_here(&p, rows, &o.path().join(format!("edge{k}")), None, &|_, _| {}).unwrap();
        // Every strip's bytes, read through it, the file's.
        for off in (0..whole.len() as u64).step_by(4093) {
            let len = 4093.min(whole.len() - off as usize);
            assert_eq!(f.read_at(off, len).unwrap(), whole[off as usize..off as usize + len], "{rows:?} at {off}");
        }
    }
}

/// Task #998: the rows a run read are kept in the cache for the runs after: the same rows read again
/// read nothing of the NAS, rows meeting them only what they lack, and every read gives the file's
/// bytes; with no room past the room target they go to the scratch alone; room-making's freeing of
/// a span (one not held) has it read again.
#[test]
fn canopy_rows_kept_in_the_cache_are_read_there() {
    let o = tempfile::tempdir().unwrap();
    let p = o.path().join(chm_name(50, 0, "cover5m"));
    // (Six strips of 2500 rows stored, each a span of its own.)
    let whole =
        tiff(40000, 16, &|tx, ty| (tx == 43 && ty < 6).then(|| testkit::samples(tx, ty, 16, &testkit::cover_at)), &[]);
    std::fs::write(&p, &whole).unwrap();
    let cache = o.path().join("cache");
    let kept = squares::RowsCache::new(&cache, 0);
    let rows = o.path().join("rows");
    let same = |f: &std::sync::Arc<dyn store::range::RangeRead>| {
        for off in (0..whole.len() as u64).step_by(4093) {
            let len = 40_000.min(whole.len() - off as usize);
            assert_eq!(f.read_at(off, len).unwrap(), whole[off as usize..off as usize + len], "at {off}");
        }
    };
    let in_cache = || -> Vec<std::path::PathBuf> {
        let mut v: Vec<_> = std::fs::read_dir(cache.join("blobs/sources/canopy"))
            .map(|r| r.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        v.sort();
        v
    };
    // (No room past the target: the scratch's alone, as with no cache.)
    let full = squares::RowsCache::new(&cache, jobrt::room::MAX_TARGET);
    let (f, first) = squares::rows_here(&p, (2600, 2700), &rows, Some(&full), &|_, _| {}).unwrap();
    assert!(first > 0 && in_cache().is_empty());
    assert_eq!(std::fs::metadata(&rows).unwrap().len(), first);
    same(&f);
    std::fs::remove_file(&rows).unwrap();

    // Kept: read once.
    let (f, read) = squares::rows_here(&p, (2600, 2700), &rows, Some(&kept), &|_, _| {}).unwrap();
    assert_eq!(read, first);
    assert!(!rows.exists(), "kept in the cache, none in the scratch");
    assert_eq!(in_cache().len(), 1);
    same(&f);
    // The same rows again: nothing of the NAS.
    let (f, read) = squares::rows_here(&p, (2600, 2700), &rows, Some(&kept), &|_, _| {}).unwrap();
    assert_eq!(read, 0);
    same(&f);
    // Rows they're inside of: only what they lack; every read across the pieces.
    let (f, read) = squares::rows_here(&p, (100, 7400), &rows, Some(&kept), &|_, _| {}).unwrap();
    assert!(read > 0 && in_cache().len() >= 2, "{read} {:?}", in_cache());
    same(&f);
    drop(f);
    // Rows inside what's kept: nothing of the NAS.
    let (f, read) = squares::rows_here(&p, (2600, 2650), &rows, Some(&kept), &|_, _| {}).unwrap();
    assert_eq!(read, 0);
    same(&f);
    drop(f);
    // Freed by room-making, held no more: read again.
    for s in in_cache() {
        store::cachefile::release(&s);
        store::testkit::wait_for("a span freed", || {
            matches!(store::cachefile::try_remove(&s), store::cachefile::Removed::Freed(_)).then_some(())
        });
    }
    let (f, read) = squares::rows_here(&p, (2600, 2700), &rows, Some(&kept), &|_, _| {}).unwrap();
    assert_eq!(read, first);
    same(&f);
    for s in in_cache() {
        store::cachefile::release(&s);
    }
}

#[test]
fn a_runs_work_stays_within_the_threads_it_was_given() {
    // (The trees program's `--workers n`: its pool of n, in which `in_order` runs the blocks; their
    // own parallel work, rows and pyramids, ran in rayon's global pool, every core, until it was
    // the pool's tasks.)
    use rayon::prelude::*;
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    for workers in [2usize, 3] {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(workers).build().unwrap();
        let (busy, most) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let ids = std::sync::Mutex::new(std::collections::BTreeSet::new());
        let work = |x: u64| {
            let now = busy.fetch_add(1, SeqCst) + 1;
            most.fetch_max(now, SeqCst);
            ids.lock().unwrap().insert(format!("{:?}", std::thread::current().id()));
            std::thread::sleep(std::time::Duration::from_micros(300));
            busy.fetch_sub(1, SeqCst);
            x
        };
        let mut sums = Vec::new();
        pool.install(|| {
            in_order(
                40,
                |i| Ok((0..32u64).into_par_iter().map(|x| work(x * i as u64)).sum::<u64>()),
                |_| {},
                |_, v| {
                    sums.push(v);
                    Ok(())
                },
            )
        })
        .unwrap();
        assert_eq!(sums, (0..40u64).map(|i| (0..32).map(|x| x * i).sum::<u64>()).collect::<Vec<_>>());
        assert!(most.load(SeqCst) <= workers, "{} at once with {workers} workers", most.load(SeqCst));
        assert!(ids.lock().unwrap().len() <= workers, "{} threads with {workers} workers", ids.lock().unwrap().len());
    }
}
