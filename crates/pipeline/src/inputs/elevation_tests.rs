//! The elevation declarations' checks, over versions held in memory and data served by a map of
//! URLs (no network): shapes, references, the data measured, and the plan a unit reads.

use super::*;
use crate::fetch::MapFetch;
use std::path::Path;

const PX: f64 = 1.0 / 3600.0;

/// A 1° GeoTIFF at 1″ (EPSG:4326) with its south-west corner at `lon`, `lat`, 36 × 36 pixels of
/// 100″ for speed (so 100″, about 3 km).
fn tile(lon: f64, lat: f64) -> Vec<u8> {
    let data = vec![12.5f32; 36 * 36];
    crate::geotiff::write_f32(
        36,
        36,
        &data,
        16,
        [lon, 100.0 * PX, 0.0, lat + 1.0, 0.0, -100.0 * PX],
        4326,
        Some(-9999.0),
    )
    .unwrap()
}

/// A `w` × `h` terrarium PNG of 0 m.
fn png(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut e = png::Encoder::new(&mut out, w, h);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(&[128u8, 0, 0].repeat((w * h) as usize)).unwrap();
    out
}

fn decl(extra: &str, data: &str) -> Vec<u8> {
    format!("name = \"x\"\nwhat = \"Road elevation, test\"\ncredit = \"Someone\"\nlicence = \"CC0-1.0\"\nsurface = \"ground\"\nheights = \"EGM2008\"\ncode = 4\n{extra}\n[data]\n{data}\n").into_bytes()
}

fn index(files: &[(&str, [f64; 4])]) -> Vec<u8> {
    let feats: Vec<serde_json::Value> = files
        .iter()
        .map(|(u, b)| serde_json::json!({"type": "Feature", "properties": {"url": u}, "geometry": {"type": "Polygon", "coordinates": [[[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]], [b[0], b[1]]]]}}))
        .collect();
    serde_json::to_vec(&serde_json::json!({"type": "FeatureCollection", "features": feats})).unwrap()
}

struct Fixture {
    files: BTreeMap<String, Arc<Vec<u8>>>,
    checks: Elevation,
    /// The stores' folder: `tiled/`'s two tiles (a dataset declared `store` is measured there).
    store: tempfile::TempDir,
}

/// The data the fixtures' declarations name, served by URL.
fn fetch() -> MapFetch {
    let mut f = MapFetch::default();
    f.0.insert("https://e.org/a.tif".into(), Some(tile(2.0, 48.0)));
    f.0.insert("https://e.org/t/N48E002.tif".into(), Some(tile(2.0, 48.0)));
    f.0.insert("https://e.org/t/N49E002.tif".into(), Some(tile(2.0, 49.0)));
    f.0.insert("https://e.org/gone.tif".into(), None);
    // (A pyramid's tile at the middle of the world, read for its size; another's of 512.)
    f.0.insert("https://e.org/12/2048/2048.png".into(), Some(png(256, 256)));
    f.0.insert("https://e.org/big/12/2048/2048.png".into(), Some(png(512, 512)));
    // Twelve 1° files without an index, along a row; one half without data (its west).
    for k in 0..12 {
        f.0.insert(format!("https://e.org/row/{k}.tif"), Some(tile(10.0 + k as f64, 20.0)));
    }
    let mut half = vec![12.5f32; 72 * 36];
    for r in 0..36 {
        for c in 0..36 {
            half[r * 72 + c] = -9999.0;
        }
    }
    f.0.insert(
        "https://e.org/half.tif".into(),
        Some(
            crate::geotiff::write_f32(
                72,
                36,
                &half,
                16,
                [30.0, 100.0 * PX, 0.0, 41.0, 0.0, -100.0 * PX],
                4326,
                Some(-9999.0),
            )
            .unwrap(),
        ),
    );
    f
}

impl Fixture {
    fn new(files: Vec<(&str, Vec<u8>)>) -> Fixture {
        let f = fetch();
        let store = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(store.path().join("tiled")).unwrap();
        std::fs::write(store.path().join("tiled/N48E002.tif"), tile(2.0, 48.0)).unwrap();
        std::fs::write(store.path().join("tiled/N49E002.tif"), tile(2.0, 49.0)).unwrap();
        let checks = Elevation {
            store: Some(store::nasfs::NasPath::local_for_test(store.path().to_path_buf())),
            ..Elevation::new(Arc::new(f), None)
        };
        Fixture { files: files.into_iter().map(|(p, b)| (p.to_string(), Arc::new(b))).collect(), checks, store }
    }

    fn whole(&self) -> Vec<Finding> {
        let rd = |p: &str| self.files.get(p).cloned().ok_or_else(|| anyhow::anyhow!("{p}: none"));
        self.checks.whole(&Version { paths: self.files.keys().cloned().collect(), read: &rd }).unwrap()
    }

    fn derive(&self) -> Derived {
        let rd = |p: &str| self.files.get(p).cloned().ok_or_else(|| anyhow::anyhow!("{p}: none"));
        self.checks
            .derive(&Version { paths: self.files.keys().cloned().collect(), read: &rd }, &BTreeSet::new())
            .unwrap()
    }

    fn checks(&self) -> Vec<String> {
        let mut v: Vec<String> = self.whole().iter().map(|f| f.check().to_string()).collect();
        for (p, b) in &self.files {
            v.extend(self.checks.file(p, b).findings.iter().map(|f| f.check().to_string()));
        }
        v.sort();
        v
    }
}

#[test]
fn the_checks_key_names_the_accepted_regions_and_sources() {
    let mut m = BTreeMap::new();
    m.insert(crate::inputs::logical(crate::inputs::regions::UNIT), "regions-index".to_string());
    m.insert(crate::inputs::logical(crate::inputs::sources::UNIT), "sources-index".to_string());
    let c = Elevation::default().context(&m);
    assert!(c.contains(&"regions regions-index".to_string()) && c.contains(&"sources sources-index".to_string()));
}

#[test]
fn excluded_cells_are_cut_from_a_coverage() {
    let f = Fixture::new(vec![(
        "cut.toml",
        decl(
            "territory = \"world\"\nexclude = [[153, 24, 155, 25]]",
            "format = \"tiles\"\nurl = \"https://e.org/{z}/{x}/{y}.png\"\nzoom = 12\nencoding = \"terrarium\"",
        ),
    )]);
    assert!(f.checks().is_empty(), "{:?}", f.whole());
    let m: Measured = serde_json::from_value(f.derive().facts["cut.toml"].0["measured"].clone()).unwrap();
    let c = declared::Cover::of(m.cover.as_deref());
    assert!(!c.contains(153.98, 24.29) && !c.contains(154.5, 24.5));
    assert!(c.contains(152.5, 24.5) && c.contains(154.5, 25.5) && c.contains(-70.0, -30.0));
    assert_eq!(c.cells.len(), 360 * 180 - 2);
    assert_eq!(m.uncut, Some(vec![declared::WORLD]));
}

/// A dataset's `regions` cut what was measured to their cells, then to their outlines inside
/// them, naming the regions' stored geometry; a region whose geometry isn't named is an error.
#[test]
fn regions_cut_a_coverage_to_their_outlines() {
    let dir = tempfile::tempdir().unwrap();
    let r =
        crate::agent::recipes::Recipe { id: "a".into(), name: "a".into(), outline: vec!["place:10.5,45.5,20".into()] };
    let cov = crate::coverage::Coverage::fixture(&[r], None, dir.path()).unwrap();
    let d2 = Declaration {
        regions: Some(vec!["a".into()]),
        ..declared::parse(&decl("", "format = \"geotiff\"\nurl = \"https://e.org/a.tif\"")).unwrap()
    };
    let m = Measured { resolution: 30.0, cover: Some(vec![[10.0, 45.0, 12.0, 46.0]]), ..Default::default() };
    let e = Elevation {
        regions: Some(cov),
        region_geometry: BTreeMap::from([("a".to_string(), "sources/inputs/regions/@geometry/a.0.rgeo".to_string())]),
        ..Elevation::default()
    };
    let got = e.scoped(&d2, m.clone()).unwrap();
    assert_eq!(got.cover, Some(vec![[10.0, 45.0, 11.0, 46.0]]));
    let o = got.outline.unwrap();
    assert_eq!(o.geometry, ["sources/inputs/regions/@geometry/a.0.rgeo"]);
    // (A circle of 20 km in its cell: the cell is an edge.)
    assert_eq!(o.edges.iter().collect::<Vec<_>>(), ["10,45"]);
    let unnamed = Elevation { region_geometry: BTreeMap::new(), ..e };
    assert!(unnamed.scoped(&d2, m).unwrap_err().contains("stored geometry"));
}

#[test]
fn good_declarations_are_measured_from_their_data() {
    let f = Fixture::new(vec![
        ("single.toml", decl("", "format = \"geotiff\"\nurl = \"https://e.org/a.tif\"")),
        ("tiled.toml", decl("rank = 1", "format = \"geotiff\"\nindex = \"tiled.geojson\"\nstore = true")),
        (
            "tiled.geojson",
            index(&[
                ("https://e.org/t/N48E002.tif", [2.0, 48.0, 3.0, 49.0]),
                ("https://e.org/t/N49E002.tif", [2.0, 49.0, 3.0, 50.0]),
            ]),
        ),
        (
            "pyramid.toml",
            decl(
                "territory = \"world\"\nrank = 2",
                "format = \"tiles\"\nurl = \"https://e.org/{z}/{x}/{y}.png\"\nzoom = 12\nencoding = \"terrarium\"",
            ),
        ),
    ]);
    assert!(f.checks().is_empty(), "{:?}", f.whole());
    let d = f.derive();
    let m: Measured = serde_json::from_value(d.facts["tiled.toml"].0["measured"].clone()).unwrap();
    assert_eq!(m.crs, Some(4326));
    assert!((m.resolution - 100.0 * PX * crate::dem::generic::M_PER_DEG).abs() < 1e-6);
    // Its cover: the 1° cells its footprints meet, edges included.
    assert_eq!(m.cover, Some(vec![[2.0, 48.0, 4.0, 51.0]]));
    assert_eq!(d.facts["pyramid.toml"].0["extent"], "world");
    // The plan a unit near Paris reads: in their order, the files near it alone.
    let d2 = tempfile::tempdir().unwrap();
    let mut files = BTreeMap::new();
    for (p, b) in &f.files {
        let c = f.checks.file(p, b);
        let mut facts = c.facts.clone();
        if let Some((more, _)) = d.facts.get(p) {
            facts.extend(more.clone());
        }
        let copy = format!("sources/inputs/elevation/{p}");
        std::fs::create_dir_all(d2.path().join(&copy).parent().unwrap()).unwrap();
        std::fs::write(d2.path().join(&copy), &**b).unwrap();
        files.insert(
            p.clone(),
            super::super::FileEntry {
                file: copy,
                size: b.len() as u64,
                keyed: c.keyed.unwrap_or_else(|| "-".into()),
                facts,
            },
        );
    }
    let ix = super::super::Index {
        fmt: 1,
        unit: UNIT.into(),
        checks: "elevation 2".into(),
        files,
        accepted: BTreeMap::new(),
    };
    let name = "sources/inputs/elevation/@index.0000000000000000.json";
    std::fs::write(d2.path().join(name), serde_json::to_vec(&ix).unwrap()).unwrap();
    let a = super::super::open(
        &store::nasfs::NasRoot::local_for_test(d2.path()),
        &BTreeMap::from([(super::super::logical(UNIT), name.to_string())]),
        UNIT,
    )
    .unwrap()
    .unwrap();
    let plan = declared::plan(&a, [2.3, 48.8, 2.4, 48.9]).unwrap();
    let ids: Vec<&str> = plan.datasets.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(ids, vec!["single", "tiled", "pyramid"]);
    let declared::Source::Geotiff { files, store, .. } = &plan.datasets[1].source else { panic!() };
    assert!(*store);
    assert_eq!(files.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(), vec!["https://e.org/t/N48E002.tif"]);
    // Far from them, the world's alone.
    let far = declared::plan(&a, [100.0, 10.0, 100.1, 10.1]).unwrap();
    assert_eq!(far.datasets.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), vec!["pyramid"]);
    assert_eq!(far.tag, plan.tag);
}

#[test]
fn what_the_checks_refuse_and_warn_of() {
    let f = Fixture::new(vec![
        // A reference unknown, a CRS's file gone, an index missing, a folder of no dataset, an
        // index no one names, a stray file, and two datasets of one rank and resolution meeting.
        ("bad.toml", decl("", "format = \"geotiff\"\nurl = \"https://e.org/a.tif\"")),
        ("gone.toml", decl("", "format = \"geotiff\"\nurl = \"https://e.org/gone.tif\"")),
        ("lost.toml", decl("", "format = \"geotiff\"\nindex = \"nowhere.geojson\"")),
        ("twin.toml", decl("", "format = \"geotiff\"\nurl = \"https://e.org/a.tif\"")),
        ("stray/x.tif", b"II*\0".to_vec()),
        ("spare.geojson", index(&[("https://e.org/a.tif", [2.0, 48.0, 3.0, 49.0])])),
        ("notes.txt", b"hello".to_vec()),
        (
            "ngvd.toml",
            String::from_utf8(decl("", "format = \"geotiff\"\nurl = \"https://e.org/a.tif\""))
                .unwrap()
                .replace("EGM2008", "NGVD29")
                .into_bytes(),
        ),
    ]);
    let got = f.checks();
    assert_eq!(got, vec![DATA, DECL, MISSING, ORPHAN, ORPHAN, SHAPE, TIE], "{:?}", f.whole());
    // The tie names both; ids are stable.
    let tie = f.whole().into_iter().find(|x| x.check() == TIE).unwrap();
    assert_eq!(tie.files, vec!["bad.toml".to_string(), "twin.toml".to_string()]);
    assert_eq!(tie.id, f.whole().into_iter().find(|x| x.check() == TIE).unwrap().id);
}

/// One label per code: two datasets of one code labelled apart are an error naming both; labelled
/// alike (or by the same `name`), none. A code past where the server counts (roadcore::NDEM) is
/// refused.
#[test]
fn one_label_per_code_and_codes_the_server_counts() {
    let url = "format = \"geotiff\"\nurl = \"https://e.org/a.tif\"";
    let f = Fixture::new(vec![
        ("a.toml", decl("label = \"A (30 m)\"", url)),
        ("b.toml", decl("label = \"B (30 m)\"\nrank = 1", url)),
    ]);
    let apart: Vec<_> = f.whole().into_iter().filter(|x| x.check() == DECL).collect();
    assert_eq!(apart.len(), 1, "{:?}", f.whole());
    assert_eq!(apart[0].files, vec!["a.toml".to_string(), "b.toml".to_string()]);
    let f = Fixture::new(vec![
        ("a.toml", decl("label = \"A (30 m)\"", url)),
        ("b.toml", decl("label = \"A (30 m)\"\nrank = 1", url)),
    ]);
    assert!(!f.checks().iter().any(|c| c == DECL), "{:?}", f.whole());
    let over = String::from_utf8(decl("", url)).unwrap().replace("code = 4", &format!("code = {}", roadcore::NDEM));
    assert!(crate::dem::declared::parse(over.as_bytes()).is_err());
    let top = String::from_utf8(decl("", url)).unwrap().replace("code = 4", &format!("code = {}", roadcore::NDEM - 1));
    assert!(crate::dem::declared::parse(top.as_bytes()).is_ok());
}

/// A dataset declared `store` is measured from its stored files, its index's URLs only naming them:
/// nothing fetched (the URLs aren't served here), and a sampled file the store lacks is an error
/// naming it.
#[test]
fn a_stored_dataset_is_measured_from_the_store_alone() {
    let decl_ = decl("", "format = \"geotiff\"\nindex = \"tiled.geojson\"\nstore = true");
    let ix = index(&[
        ("https://gone.example/z.zip#N48E002.tif", [2.0, 48.0, 3.0, 49.0]),
        ("https://gone.example/z.zip#N49E002.tif", [2.0, 49.0, 3.0, 50.0]),
    ]);
    let f = Fixture::new(vec![("tiled.toml", decl_.clone()), ("tiled.geojson", ix.clone())]);
    assert!(f.checks().is_empty(), "{:?}", f.whole());
    let m: Measured = serde_json::from_value(f.derive().facts["tiled.toml"].0["measured"].clone()).unwrap();
    assert_eq!(m.crs, Some(4326));
    std::fs::remove_file(f.store.path().join("tiled/N49E002.tif")).unwrap();
    let g = Fixture {
        files: f.files.clone(),
        checks: Elevation {
            store: Some(store::nasfs::NasPath::local_for_test(f.store.path().to_path_buf())),
            ..Elevation::new(Arc::new(fetch()), None)
        },
        store: tempfile::tempdir().unwrap(),
    };
    let why: Vec<String> = g.whole().iter().map(|x| format!("{x:?}")).collect();
    assert!(why.iter().any(|w| w.contains("tile N49E002.tif isn't in the store")), "{why:?}");
}

#[test]
fn a_datasets_keyed_digest_names_its_index_and_dropped_files() {
    let ix = |files: &[(&str, [f64; 4])]| index(files);
    let one = [("https://e.org/t/N48E002.tif", [2.0, 48.0, 3.0, 49.0])];
    let two = [
        ("https://e.org/t/N48E002.tif", [2.0, 48.0, 3.0, 49.0]),
        ("https://e.org/t/N49E002.tif", [2.0, 49.0, 3.0, 50.0]),
    ];
    let keyed_of = |f: &Fixture, p: &str| f.derive().facts[p].1.clone().unwrap();
    let a = Fixture::new(vec![
        ("tiled.toml", decl("", "format = \"geotiff\"\nindex = \"tiled.geojson\"")),
        ("tiled.geojson", ix(&one)),
    ]);
    let b = Fixture::new(vec![
        ("tiled.toml", decl("", "format = \"geotiff\"\nindex = \"tiled.geojson\"")),
        ("tiled.geojson", ix(&two)),
    ]);
    assert!(a.checks().is_empty() && b.checks().is_empty(), "{:?}", b.whole());
    // The declaration the same, a tile added to its index: another digest (its units rerun).
    assert_ne!(keyed_of(&a, "tiled.toml"), keyed_of(&b, "tiled.toml"));
    assert_eq!(
        a.checks.file("tiled.toml", &a.files["tiled.toml"]).keyed,
        b.checks.file("tiled.toml", &b.files["tiled.toml"]).keyed
    );
    // Added files: one added, or one changed, likewise.
    let d = |files: Vec<(&str, Vec<u8>)>| {
        let mut v = vec![("local.toml", decl("", "format = \"geotiff\"\nfiles = [\"*.tif\"]"))];
        v.extend(files);
        Fixture::new(v)
    };
    let d1 = d(vec![("local/a.tif", tile(2.0, 48.0))]);
    let d2 = d(vec![("local/a.tif", tile(2.0, 48.0)), ("local/b.tif", tile(2.0, 49.0))]);
    let d3 = d(vec![("local/a.tif", tile(3.0, 48.0))]);
    assert!(d1.checks().is_empty() && d2.checks().is_empty(), "{:?}", d2.whole());
    let (k1, k2, k3) = (keyed_of(&d1, "local.toml"), keyed_of(&d2, "local.toml"), keyed_of(&d3, "local.toml"));
    assert!(k1 != k2 && k1 != k3 && k2 != k3);
    // A pyramid has no data beyond its declaration: its file check's digest.
    let p = Fixture::new(vec![(
        "pyramid.toml",
        decl(
            "territory = \"world\"",
            "format = \"tiles\"\nurl = \"https://e.org/{z}/{x}/{y}.png\"\nzoom = 12\nencoding = \"terrarium\"",
        ),
    )]);
    assert_eq!(Some(keyed_of(&p, "pyramid.toml")), p.checks.file("pyramid.toml", &p.files["pyramid.toml"]).keyed);
}

#[test]
fn every_file_without_an_index_is_measured_and_only_its_valid_data_covers() {
    // Twelve files by URL (more than an index's sample): each one's cell in the cover.
    let urls: Vec<String> = (0..12).map(|k| format!("\"https://e.org/row/{k}.tif\"")).collect();
    let f = Fixture::new(vec![("row.toml", decl("", &format!("format = \"geotiff\"\nurls = [{}]", urls.join(", "))))]);
    assert!(f.checks().is_empty(), "{:?}", f.whole());
    let m: Measured = serde_json::from_value(f.derive().facts["row.toml"].0["measured"].clone()).unwrap();
    assert_eq!(m.cover, Some(vec![[10.0, 20.0, 23.0, 22.0]]));
    // A file whose west half has no data: its east alone (31°–32°E, edges included), not its bounds.
    let f = Fixture::new(vec![("half.toml", decl("", "format = \"geotiff\"\nurl = \"https://e.org/half.tif\""))]);
    assert!(f.checks().is_empty(), "{:?}", f.whole());
    let m: Measured = serde_json::from_value(f.derive().facts["half.toml"].0["measured"].clone()).unwrap();
    assert_eq!(m.cover, Some(vec![[31.0, 40.0, 33.0, 42.0]]));
}

#[test]
fn a_pyramid_of_tiles_not_256_pixels_is_refused() {
    let f = Fixture::new(vec![(
        "big.toml",
        decl(
            "territory = \"world\"",
            "format = \"tiles\"\nurl = \"https://e.org/big/{z}/{x}/{y}.png\"\nzoom = 12\nencoding = \"terrarium\"",
        ),
    )]);
    let w = f.whole();
    assert_eq!(w.iter().map(|x| x.check()).collect::<Vec<_>>(), vec![DATA]);
    assert!(w[0].message.contains("512 × 512"), "{}", w[0].message);
}

const APP: &str = "20261016-1200-abcdef0";

#[test]
fn the_switch_names_the_accepted_index_and_a_published_app_only() {
    let mut f = Fixture::new(vec![]);
    f.checks.app = Some(APP.into());
    let b = f
        .checks
        .switch(
            &store::nasfs::NasRoot::local_for_test(Path::new("/nonexistent")),
            "sources/inputs/elevation/@index.0123456789abcdef.json",
        )
        .unwrap()
        .expect("switched at once");
    let v: serde_json::Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v, serde_json::json!({ "index": "sources/inputs/elevation/@index.0123456789abcdef.json", "app": APP }));
    // A development build records none: it would bar no app, for good.
    f.checks.app = Some("development".into());
    assert!(f
        .checks
        .switch(
            &store::nasfs::NasRoot::local_for_test(Path::new("/nonexistent")),
            "sources/inputs/elevation/@index.0123456789abcdef.json"
        )
        .unwrap()
        .is_none());
}

#[test]
fn the_check_job_records_the_switch_once_with_the_first_accepted_index() {
    TEST_PREPARED.with(|t| *t.borrow_mut() = Some((Arc::new(fetch()), APP.to_string())));
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    std::fs::create_dir_all(root.join("state/build")).unwrap();
    std::fs::write(root.join("state/build/manifest.json"), b"{}").unwrap();
    let dir = root.join("inputs/elevation");
    std::fs::create_dir_all(&dir).unwrap();
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    let add = |p: &str, b: &[u8]| {
        std::fs::write(dir.join(p), b).unwrap();
        std::fs::File::options().write(true).open(dir.join(p)).unwrap().set_modified(old).unwrap();
    };
    let manifest = || -> BTreeMap<String, String> {
        crate::out::read_record(&store::nasfs::NasPath::local_for_test(&root.join("state/build/manifest.json")))
            .unwrap()
    };
    let run = || {
        let mut out =
            crate::out::Out::unbound(&store::nasfs::NasRoot::local_for_test(root), &root.join("scratch")).unwrap();
        crate::inputs::gate::run(&mut out, UNIT, false, None).unwrap()
    };
    add("single.toml", &decl("", "format = \"geotiff\"\nurl = \"https://e.org/a.tif\""));
    let said = run();
    let m = manifest();
    let first = m.get(&super::super::logical(UNIT)).cloned().unwrap_or_else(|| panic!("not taken in: {said}"));
    assert!(switched(&m), "{said}");
    let sw = m[&super::super::switched_logical(UNIT)].clone();
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join(&sw)).unwrap()).unwrap();
    assert_eq!(v, serde_json::json!({ "index": first, "app": APP }));
    // A later version replaces the index; the switch stays as it was.
    add("second.toml", &decl("rank = 1", "format = \"geotiff\"\nurl = \"https://e.org/t/N48E002.tif\""));
    let said = run();
    let m = manifest();
    assert_ne!(m[&super::super::logical(UNIT)], first, "{said}");
    assert_eq!(m[&super::super::switched_logical(UNIT)], sw);
    TEST_PREPARED.with(|t| *t.borrow_mut() = None);
}

/// A geoid source linking `from` to EGM2008 by the grid at `url`.
fn geoid_grid(id: &str, from: &str, url: &str) -> crate::inputs::sources::Source {
    let t = format!(
        "name = \"N\"\ncredit = \"C\"\nlicence = \"CC0-1.0\"\nterritory = \"world\"\n[data]\nrole = \"geoid\"\nkind = \"data\"\n\
         format = \"geoid-grid\"\nurl = \"{url}\"\n[data.params]\nfrom = \"{from}\"\nto = \"EGM2008\""
    );
    let d = crate::inputs::sources::parse(t.as_bytes()).unwrap();
    crate::inputs::sources::Source { id: id.into(), data: d.data, keyed: "g".into(), index: Vec::new() }
}

/// `single.toml` (its data in 1° cell 2° E 48° N, its coverage that cell's and those beside it, 2..4° E 48..50° N) in heights NAVD88, checked with geoid sources
/// `geoid`, one grid served at https://g.org/w.tif spanning `grid` (w, s, e, n): its el-geoid findings.
fn geoid_findings(geoid: Vec<crate::inputs::sources::Source>, grid: [f64; 4]) -> Vec<Finding> {
    let d = String::from_utf8(decl("", "format = \"geotiff\"\nurl = \"https://e.org/a.tif\"")).unwrap();
    let mut f = Fixture::new(vec![("single.toml", d.replace("EGM2008", "NAVD88").into_bytes())]);
    let mut fetch = fetch();
    let (w, h) = (((grid[2] - grid[0]) * 10.0) as u32, ((grid[3] - grid[1]) * 10.0) as u32);
    let g = crate::geotiff::write_f32(
        w,
        h,
        &vec![-30.0f32; (w * h) as usize],
        16,
        [grid[0], 0.1, 0.0, grid[3], 0.0, -0.1],
        4326,
        None,
    )
    .unwrap();
    fetch.0.insert("https://g.org/w.tif".into(), Some(g));
    f.checks = Elevation { geoid: Some(geoid), store: f.checks.store.take(), ..Elevation::new(Arc::new(fetch), None) };
    f.whole().into_iter().filter(|x| x.check() == GEOID).collect()
}

#[test]
fn heights_no_geoid_chain_reaches_are_an_error() {
    let got = geoid_findings(vec![geoid_grid("g", "JGD2011", "https://g.org/w.tif")], [0.0, 40.0, 10.0, 50.0]);
    assert!(got.len() == 1 && got[0].level == Level::Error, "{got:?}");
    assert!(got[0].message.contains("NAVD88") && got[0].message.contains("EGM2008"), "{}", got[0].message);
}

#[test]
fn heights_whose_grids_reach_part_of_the_data_are_a_warning_naming_the_rest() {
    // (The grid stops at 3.5° E: the eastern cells' eastern halves aren't reached.)
    let got = geoid_findings(vec![geoid_grid("g", "NAVD88", "https://g.org/w.tif")], [1.0, 47.0, 3.5, 51.0]);
    assert!(got.len() == 1 && got[0].level == Level::Warning, "{got:?}");
    let m = &got[0].message;
    assert!(m.contains("cells 3..4° E 48..50° N:"), "{m}");
    // Half a degree of longitude at 48..50° N: 111.2 × (73.7 + 72.2) / 2 km².
    let km2: f64 = m.split(" km²").next().unwrap().rsplit(' ').next().unwrap().parse().unwrap();
    assert!((km2 - 8110.0).abs() < 60.0, "{m}");
}

#[test]
fn heights_whose_grids_reach_all_the_data_pass() {
    let got = geoid_findings(vec![geoid_grid("g", "NAVD88", "https://g.org/w.tif")], [0.0, 40.0, 10.0, 50.0]);
    assert!(got.is_empty(), "{got:?}");
    // Not converted at all (already EGM2008): nothing to find either.
    let d = decl("", "format = \"geotiff\"\nurl = \"https://e.org/a.tif\"");
    let mut f = Fixture::new(vec![("single.toml", d)]);
    f.checks.geoid = Some(Vec::new());
    assert!(f.whole().iter().all(|x| x.check() != GEOID));
}

#[test]
fn an_unchanged_dataset_whose_grids_stop_reaching_it_is_held_as_accepted() {
    // (single.toml in NAVD88, taken in with a grid reaching all of it.)
    let d = String::from_utf8(decl("", "format = \"geotiff\"\nurl = \"https://e.org/a.tif\"")).unwrap();
    let b = d.replace("EGM2008", "NAVD88").into_bytes();
    let listing = crate::inputs::Listing {
        files: [("single.toml".to_string(), (b.len() as u64, 1))].into(),
        ..Default::default()
    };
    let checks = |grid: [f64; 4]| {
        let mut fetch = fetch();
        let (w, h) = (((grid[2] - grid[0]) * 10.0) as u32, ((grid[3] - grid[1]) * 10.0) as u32);
        let g = crate::geotiff::write_f32(
            w,
            h,
            &vec![-30.0f32; (w * h) as usize],
            16,
            [grid[0], 0.1, 0.0, grid[3], 0.0, -0.1],
            4326,
            None,
        )
        .unwrap();
        fetch.0.insert("https://g.org/w.tif".into(), Some(g));
        Elevation {
            geoid: Some(vec![geoid_grid("g", "NAVD88", "https://g.org/w.tif")]),
            ..Elevation::new(Arc::new(fetch), None)
        }
    };
    let copies: std::sync::Mutex<BTreeMap<String, Vec<u8>>> = Default::default();
    let read_new = |_: &str| Ok(b.clone());
    let read_old = |e: &crate::inputs::FileEntry| {
        use anyhow::Context as _;
        copies.lock().unwrap().get(&e.file).cloned().context("no copy")
    };
    let decide = |c: &Elevation, prev: Option<&crate::inputs::Index>, prev_listed: &BTreeMap<String, (u64, u64)>| {
        let d = crate::inputs::gate::decide(&crate::inputs::gate::Given {
            checks: c,
            prev,
            prev_listed,
            listing: &listing,
            unsettled: &BTreeSet::new(),
            accepted: &BTreeSet::new(),
            full: false,
            read_new: &read_new,
            read_old: &read_old,
        })
        .unwrap();
        copies.lock().unwrap().extend(d.store.iter().map(|(n, b)| (n.clone(), b.to_vec())));
        d
    };
    let first = decide(&checks([0.0, 40.0, 10.0, 50.0]), None, &BTreeMap::new());
    assert!(first.report.is_none() && first.index.files.contains_key("single.toml"), "{:?}", first.report);
    // The sources now accepted reach only part of it: nothing changed in the folder, and it's held,
    // the version in use kept.
    let again = decide(&checks([1.0, 47.0, 3.5, 51.0]), Some(&first.index), &first.listed);
    let r = again.report.expect("held");
    assert_eq!(r.held, vec!["single.toml".to_string()]);
    assert!(r.findings.iter().any(|f| f.check() == GEOID && f.level == Level::Warning), "{:?}", r.findings);
    assert!(!again.changed && again.index.files == first.index.files);
}
