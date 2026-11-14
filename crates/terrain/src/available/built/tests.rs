use super::*;
use codecs::available::{Grid, Tile};
use std::path::Path;
use store::nasfs::NasRoot;

fn out(root: &Path) -> Out {
    Out::unbound(&NasRoot::local_for_test(root), &root.join("scratch")).unwrap()
}

/// A dataset as `datasets` makes one, by hand.
fn dataset(id: &str, cover: Option<[f64; 4]>, has: Has) -> Dataset {
    let boxes = cover.map(|b| vec![b]);
    Dataset {
        id: id.into(),
        line: format!("{id} k1"),
        measured: declared::Measured::default(),
        cover: Cover::of(boxes.as_deref()),
        has,
        info: json!({"id": id}),
    }
}

fn everywhere(id: &str) -> Dataset {
    dataset(id, None, Has::Everywhere)
}

/// The z10 tile holding a point.
fn z10(lon: f64, lat: f64) -> (u32, u32) {
    roadcore::geo::tile_of(roadcore::geo::merc(lon, lat), BUILT_MAXZ)
}

fn run(root: &Path, ds: &[Dataset]) -> Made {
    let mut o = out(root);
    let m = build(&mut o, ds, &|_, _| {}).unwrap();
    o.save().unwrap();
    m
}

/// The stored tile z/x/y of a layer, from the records at `root`.
fn stored_tile(root: &Path, layer: &str, z: u8, x: u32, y: u32) -> Option<Tile> {
    let o = out(root);
    let (_, _, grid) = fmt::of_layer(layer).unwrap();
    let (scope, key) = fmt::pack_of(grid, z, x, y)?;
    let c = o.get(&format!("layers/{layer}/{scope}/{}", key.replace('/', "-")))?;
    let b = o.path(c).read().unwrap();
    let idx = store::pack::PackIndex::parse_bytes(&b).unwrap();
    let e = idx.find(z, x, y)?;
    Some(Tile::decode(&b[e.offset as usize..][..e.len as usize]).unwrap())
}

/// The datasets the built set says the terrain used at a point (its z12 pixel, as a click reads it).
fn at(root: &Path, lon: f64, lat: f64) -> Vec<String> {
    let (x, y) = roadcore::geo::merc(lon, lat);
    let w = (SIDE as f64) * f64::from(1u32 << fmt::MAXZ);
    let layer = fmt::layer(THEME, fmt::BUILT);
    let mut get = |z, x, y| Ok(stored_tile(root, &layer, z, x, y));
    fmt::pieces_at(Grid::Mercator, (x * w) as u64, (y * w) as u64, &mut get).unwrap()
}

fn at_pole(root: &Path, pole: Pole, lon: f64, lat: f64) -> Vec<String> {
    let p = super::super::polar_px(pole, lon, lat);
    let layer = fmt::polar_layer(THEME, fmt::BUILT, pole.letter());
    let mut get = |z, x, y| Ok(stored_tile(root, &layer, z, x, y));
    fmt::pieces_at(Grid::Polar(pole.letter()), p[0] as u64, p[1] as u64, &mut get).unwrap()
}

/// A one-degree mask file of 360 × 360 pixels in the store (`sources/fetched/<id>/<name>`), its
/// value `v(column, row)`, its west and north edges `w`, `n`.
fn mask_file(root: &Path, id: &str, name: &str, w: f64, n: f64, v: &dyn Fn(u32, u32) -> f32) {
    let px: Vec<f32> = (0..360 * 360).map(|k| v(k % 360, k / 360)).collect();
    let gt = [w, 1.0 / 360.0, 0.0, n, 0.0, -1.0 / 360.0];
    let dir = root.join("sources/fetched").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), buildstore::geotiff::write_f32(360, 360, &px, 256, gt, 4326, None).unwrap())
        .unwrap();
}

/// The order decides, where each dataset's data is: a fine dataset in its presence tile alone, in
/// its footprint, the one after it elsewhere; stored to z10, a click's z12 pixel its z10 pixel's.
#[test]
fn the_first_dataset_with_data_is_the_one_used() {
    let d = tempfile::tempdir().unwrap();
    let (tx, ty) = z10(2.25, 44.25);
    let b = roadcore::geo::tile_box(BUILT_MAXZ, tx, ty);
    let fine = dataset(
        "fine",
        None,
        Has::Present {
            crs: 4326,
            footprints: vec![[b[0] - 0.5, b[1] - 0.5, (b[0] + b[2]) / 2.0, b[3] + 0.5]],
            tiles: Some([(tx, ty)].into_iter().collect()),
        },
    );
    let m = run(d.path(), &[fine, everywhere("world")]);
    assert!(m.root && !m.remade.is_empty(), "{m:?}");
    let (cx, cy) = ((b[0] * 3.0 + b[2]) / 4.0, (b[1] + b[3]) / 2.0);
    assert_eq!(at(d.path(), cx, cy), ["fine"], "in its presence tile and footprint");
    assert_eq!(at(d.path(), (b[0] + b[2] * 3.0) / 4.0, cy), ["world"], "past its footprint");
    assert_eq!(at(d.path(), b[0] - 0.2, cy), ["world"], "in its footprint, not its presence");
    assert_eq!(at(d.path(), -100.0, 40.0), ["world"]);
    // (No tile past z10 is stored: z11 and z12 are its pixels' parts.)
    let o = out(d.path());
    let layer = fmt::layer(THEME, fmt::BUILT);
    let packs =
        o.manifest.iter().filter(|(l, _)| l.starts_with(&format!("layers/{layer}/")) && l.split('/').count() == 4);
    for (l, c) in packs {
        let b = o.path(c).read().unwrap();
        let idx = store::pack::PackIndex::parse_bytes(&b).unwrap();
        assert!(idx.entries.iter().all(|e| e.zxy().0 <= BUILT_MAXZ), "{l}");
    }
    // (The datasets described beside the tiles, in the order.)
    let ds: Vec<Value> =
        serde_json::from_slice(&o.path(o.get(&fmt::datasets_logical(THEME)).unwrap()).read().unwrap()).unwrap();
    assert_eq!(ds.iter().map(|d| d["id"].as_str().unwrap()).collect::<Vec<_>>(), ["fine", "world"]);
}

/// A stored dataset's fill mask: a void (3 and up) is the next dataset's, as the sampler takes it;
/// its coverage cut: outside it, the next one's.
#[test]
fn a_fill_masks_voids_and_a_coverage_cut_are_the_next_datasets() {
    let d = tempfile::tempdir().unwrap();
    mask_file(d.path(), "glo", "T_FLM.tif", 10.0, 41.0, &|c, r| {
        if (100..200).contains(&c) && (100..200).contains(&r) {
            5.0
        } else {
            1.0
        }
    });
    let glo = dataset(
        "glo",
        None,
        Has::Stored {
            crs: 4326,
            dir: NasRoot::local_for_test(d.path()).join("sources/fetched/glo"),
            files: vec![("https://e.org/T_FLM.tif".into(), [10.0, 40.0, 11.0, 41.0], true)],
        },
    );
    let cut = dataset("cut", Some([5.0, 45.0, 6.0, 46.0]), Has::Everywhere);
    run(d.path(), &[cut, glo, everywhere("world")]);
    assert_eq!(at(d.path(), 10.1, 40.9), ["glo"]);
    assert_eq!(at(d.path(), 10.0 + 150.5 / 360.0, 41.0 - 150.5 / 360.0), ["world"], "a void");
    assert_eq!(at(d.path(), 11.5, 40.5), ["world"], "past its files");
    assert_eq!(at(d.path(), 5.5, 45.5), ["cut"]);
    assert_eq!(at(d.path(), 6.5, 45.5), ["world"], "past its coverage");
    // (Zoomed out, a pixel's set the union under it: glo and world over the void.)
    let (x, y) = roadcore::geo::tile_of(roadcore::geo::merc(10.5, 40.5), 3);
    let t = stored_tile(d.path(), &fmt::layer(THEME, fmt::BUILT), 3, x, y).unwrap();
    assert!(t.sets.iter().any(|s| s.len() == 2), "{:?}", t.sets);
}

/// Each z6 tile keyed by the datasets meeting it: nothing moved, nothing made again; a dataset's
/// presence moved, only its z6 tile.
#[test]
fn only_the_z6_tiles_a_dataset_moved_in_are_made_again() {
    let d = tempfile::tempdir().unwrap();
    let (tx, ty) = z10(2.25, 44.25);
    let fine = |tiles: &[(u32, u32)]| {
        let mut f = dataset(
            "fine",
            None,
            Has::Present {
                crs: 4326,
                footprints: vec![[1.0, 43.0, 4.0, 46.0]],
                tiles: Some(tiles.iter().copied().collect()),
            },
        );
        f.measured.presence = Some(tiles.iter().map(|t| [t.0, t.1]).collect());
        f
    };
    run(d.path(), &[fine(&[(tx, ty)]), everywhere("world")]);
    let m = run(d.path(), &[fine(&[(tx, ty)]), everywhere("world")]);
    assert!(m.remade.is_empty() && !m.root && m.poles.is_empty(), "{m:?}");
    let m = run(d.path(), &[fine(&[(tx, ty), (tx + 1, ty)]), everywhere("world")]);
    assert_eq!(m.remade, [format!("6/{}/{}", tx >> 4, ty >> 4)]);
    assert_eq!(at(d.path(), roadcore::geo::tile_box(BUILT_MAXZ, tx + 1, ty)[0] + 0.01, 44.25), ["fine"]);
    // (Made from nothing, the same files.)
    let fresh = tempfile::tempdir().unwrap();
    run(fresh.path(), &[fine(&[(tx, ty), (tx + 1, ty)]), everywhere("world")]);
    let packs = |r: &Path| -> BTreeMap<String, String> {
        out(r).manifest.into_iter().filter(|(l, _)| l.starts_with("layers/available-built-")).collect()
    };
    assert_eq!(packs(d.path()), packs(fresh.path()));
}

/// A pause between z6 tiles keeps those made (the records saved, the job told it's paused): the
/// next run makes the rest, then the root and the caps, ending with the files of a run not paused.
#[test]
fn a_pause_keeps_the_z6_tiles_made_and_the_next_run_makes_the_rest() {
    let d = tempfile::tempdir().unwrap();
    let ds = [everywhere("world")];
    let e = build_with(&mut out(d.path()), &ds, &|_, _| {}, &|| true).unwrap_err();
    assert!(jobrt::control::is_paused(&e), "{e:#}");
    let m = run(d.path(), &ds);
    assert_eq!(m.remade.len(), 4095, "the first kept");
    assert!(m.root && m.poles.len() == 2, "{:?}", m.poles);
    let fresh = tempfile::tempdir().unwrap();
    run(fresh.path(), &ds);
    let files = |r: &Path| -> BTreeMap<String, String> {
        out(r).manifest.into_iter().filter(|(l, _)| l.contains("/available-built-")).collect()
    };
    assert_eq!(files(d.path()), files(fresh.path()));
    // (Paused again and again, at safe points all through, over a fine dataset's packs: the same
    // files in the end, no z6 tile lost or made twice.)
    let (tx, ty) = z10(2.25, 44.25);
    let b = roadcore::geo::tile_box(BUILT_MAXZ, tx, ty);
    let fine = || {
        dataset(
            "fine",
            None,
            Has::Present {
                crs: 4326,
                footprints: vec![[b[0] - 0.5, b[1] - 0.5, (b[0] + b[2]) / 2.0, b[3] + 0.5]],
                tiles: Some([(tx, ty)].into_iter().collect()),
            },
        )
    };
    let ds = [fine(), everywhere("world")];
    let paused = tempfile::tempdir().unwrap();
    let n = std::cell::Cell::new(0u32);
    let pausing = || {
        n.set(n.get() + 1);
        n.get() % 701 == 0
    };
    let (mut pauses, mut made) = (0, 0);
    loop {
        let mut o = out(paused.path());
        match build_with(&mut o, &ds, &|_, _| {}, &pausing) {
            Ok(m) => {
                o.save().unwrap();
                made += m.remade.len();
                break;
            }
            Err(e) => {
                assert!(jobrt::control::is_paused(&e), "{e:#}");
                pauses += 1;
                made += 701;
            }
        }
    }
    assert!(pauses >= 5, "{pauses}");
    assert_eq!(made, 4096, "each z6 tile made once");
    let whole = tempfile::tempdir().unwrap();
    run(whole.path(), &ds);
    assert_eq!(files(paused.path()), files(whole.path()));
}

/// Past 85.05° the pole's square: a stored dataset holding the pole there; the Web Mercator
/// plane's datasets up to its edge.
#[test]
fn the_polar_caps_take_the_datasets_there() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("sources/fetched/ice");
    std::fs::create_dir_all(&dir).unwrap();
    let px = vec![1.0f32; 360 * 40];
    let gt = [-180.0, 1.0, 0.0, -86.0, 0.0, -0.1];
    std::fs::write(
        dir.join("S.tif"),
        buildstore::geotiff::write_f32(360, 40, &px, 256, gt, 4326, Some(-9999.0)).unwrap(),
    )
    .unwrap();
    let ice = dataset(
        "ice",
        None,
        Has::Stored {
            crs: 4326,
            dir: NasRoot::local_for_test(d.path()).join("sources/fetched/ice"),
            files: vec![("https://e.org/S.tif".into(), [-180.0, -90.0, 180.0, -86.0], false)],
        },
    );
    let m = run(d.path(), &[ice, everywhere("world")]);
    assert_eq!(m.poles, ['n', 's']);
    assert_eq!(at_pole(d.path(), Pole::South, 30.0, -88.0), ["ice"]);
    assert_eq!(at_pole(d.path(), Pole::South, 30.0, -84.0), ["world"]);
    assert_eq!(at_pole(d.path(), Pole::North, 30.0, 88.0), Vec::<String>::new(), "nothing past the plane's edge");
}

/// A pyramid's tile with data in its store is the dataset's; one the server hasn't, or the store
/// lacks, isn't.
#[test]
fn a_pyramids_stored_tiles_with_data_are_its_own() {
    let (x, y) = roadcore::geo::tile_of(roadcore::geo::merc(139.7, 35.7), 15);
    let data: HashMap<u64, bool> =
        [(roadcore::archive::tile_key(15, x, y), true), (roadcore::archive::tile_key(15, x + 1, y), false)].into();
    let n = Near::Tiles { zoom: 15, data };
    let b = roadcore::geo::tile_box(15, x, y);
    let mid = |dx: f64| ((b[0] + b[2]) / 2.0 + dx * (b[2] - b[0]), (b[1] + b[3]) / 2.0);
    assert!(n.has(mid(0.0).0, mid(0.0).1));
    assert!(!n.has(mid(1.0).0, mid(1.0).1), "the server hasn't it");
    assert!(!n.has(mid(2.0).0, mid(2.0).1), "the store lacks it");
}

/// A stored file of `px` pixels a side, each `deg / px` degrees, in the store
/// (`sources/fetched/<id>/<name>`), its value `v(column, row)`, its west and north edges `w`, `n`.
fn stored_file(
    root: &Path,
    id: &str,
    name: &str,
    [w, n, deg]: [f64; 3],
    nodata: Option<f64>,
    v: &dyn Fn(u32, u32) -> f32,
) {
    let px = 360u32;
    let vals: Vec<f32> = (0..px * px).map(|k| v(k % px, k / px)).collect();
    let gt = [w, deg / f64::from(px), 0.0, n, 0.0, -deg / f64::from(px)];
    let dir = root.join("sources/fetched").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), buildstore::geotiff::write_f32(px, px, &vals, 256, gt, 4326, nodata).unwrap())
        .unwrap();
}

/// The built set's rule is the terrain's sampler's (inputs::dem::generic, the one sampler the
/// terrain's tiles are read through): over stored datasets made by hand (a coverage cut, nodata, a
/// fill mask's voids, two overlapping files of one dataset, the first's void the second's data),
/// each point's dataset as the built set reads it is the one the sampler takes its height from.
#[test]
fn each_points_dataset_is_the_samplers_first() {
    use inputs::dem::declared::{Dataset as Planned, Format, Plan};
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let url = |n: &str| format!("https://e.org/{n}");
    // ("cut": a 3° file, nodata west of 10.67°E, its coverage the cell 10–11°E 40–41°N alone.)
    stored_file(root, "cut", "C.tif", [9.0, 42.0, 3.0], Some(-9999.0), &|c, _| if c < 200 { -9999.0 } else { 50.0 });
    // ("glo": A, its fill mask's voids in a square; B, from 10.25°E, under the square's east part.)
    stored_file(root, "glo", "A.tif", [10.0, 41.0, 1.0], None, &|_, _| 100.0);
    stored_file(root, "glo", "A_FLM.tif", [10.0, 41.0, 1.0], None, &|c, r| {
        if (50..200).contains(&c) && (100..200).contains(&r) {
            5.0
        } else {
            1.0
        }
    });
    stored_file(root, "glo", "B.tif", [10.25, 41.0, 1.0], None, &|_, _| 200.0);
    // ("base": 3° of data everywhere.)
    stored_file(root, "base", "D.tif", [9.0, 42.0, 3.0], None, &|_, _| 10.0);
    let (a, b, big) = ([10.0, 40.0, 11.0, 41.0], [10.25, 40.0, 11.25, 41.0], [9.0, 39.0, 12.0, 42.0]);
    let dir = |id: &str| NasRoot::local_for_test(root).join("sources/fetched").join(id);
    let cut = dataset(
        "cut",
        Some([10.0, 40.0, 11.0, 41.0]),
        Has::Stored { crs: 4326, dir: dir("cut"), files: vec![(url("C.tif"), big, false)] },
    );
    let glo = dataset(
        "glo",
        None,
        Has::Stored { crs: 4326, dir: dir("glo"), files: vec![(url("A_FLM.tif"), a, true), (url("B.tif"), b, false)] },
    );
    let base =
        dataset("base", None, Has::Stored { crs: 4326, dir: dir("base"), files: vec![(url("D.tif"), big, false)] });
    let ds = [cut, glo, base];
    let geotiff = |files: Vec<(String, Option<[f64; 4]>)>, masks| Format::Geotiff {
        crs: 4326,
        read_resolution: None,
        store: true,
        files,
        masks,
    };
    let planned = |id: &str, cover: Option<Vec<[f64; 4]>>, source| Planned {
        id: id.into(),
        code: 1,
        resolution: 30.0,
        cover,
        source,
        convert: None,
        outline: None,
    };
    let plan = Plan {
        fmt: 1,
        version: "t".into(),
        tag: "1".into(),
        datasets: vec![
            planned("cut", Some(vec![[10.0, 40.0, 11.0, 41.0]]), geotiff(vec![(url("C.tif"), Some(big))], vec![None])),
            planned(
                "glo",
                None,
                geotiff(vec![(url("A.tif"), Some(a)), (url("B.tif"), Some(b))], vec![Some(url("A_FLM.tif")), None]),
            ),
            planned("base", None, geotiff(vec![(url("D.tif"), Some(big))], vec![None])),
        ],
    };
    // (Points over all of it, none on a pixel's edge.)
    let mut seed = 7u64;
    let mut rnd = move || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };
    let (lon, lat): (Vec<f64>, Vec<f64>) = (0..20_000).map(|_| (9.6 + 2.0 * rnd(), 39.6 + 1.8 * rnd())).unzip();
    let f = buildstore::fetch::MapFetch::default();
    let o = inputs::dem::generic::Opts {
        fetch: &f,
        store: Some(store::nasfs::NasPath::local_for_test(root.join("sources/fetched"))),
        local: None,
    };
    let got = inputs::dem::generic::sample(&plan, &lon, &lat, &o, &|_| {}).unwrap();
    let area = [9.6, 39.6, 11.6, 41.4];
    let near: Vec<Near> = ds.iter().map(|d| near(d, area, 1.0, None)).collect::<Result<_>>().unwrap();
    let here = Here { ds: &ds, near, one: vec![1, 2, 3] };
    let (skip, whole) = (vec![false; 3], vec![false; 3]);
    let mut seen = [0usize; 4];
    for i in 0..lon.len() {
        let built = here.at(lon[i], lat[i], &skip, &whole);
        let sampler = if got.first[i] == u16::MAX { 0 } else { u32::from(got.first[i]) + 1 };
        assert_eq!(built, sampler, "at {}, {}", lon[i], lat[i]);
        seen[built as usize] += 1;
    }
    // (Each case met: every dataset somewhere, the voids' east part B's.)
    assert!(seen[1..].iter().all(|&n| n > 100), "{seen:?}");
    let (vx, vy) = (10.0 + 170.5 / 360.0, 41.0 - 150.5 / 360.0);
    let (vw, vyw) = (10.0 + 60.5 / 360.0, vy);
    let one = |x: f64, y: f64| here.at(x, y, &skip, &whole);
    assert_eq!((one(vx, vy), one(vw, vyw)), (2, 3), "a void over B is B's (glo), west of B the next dataset's");
}
