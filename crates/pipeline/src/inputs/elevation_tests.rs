//! The elevation declarations' checks, over versions held in memory and data served by a map of
//! URLs (no network): shapes, references, the data measured, and the plan a unit reads.

use super::*;
use crate::fetch::MapFetch;

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
        let checks = Elevation { store: Some(store.path().to_path_buf()), ..Elevation::new(Arc::new(f), None) };
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
fn the_checks_key_names_the_accepted_regions() {
    let mut m = BTreeMap::new();
    m.insert(crate::inputs::logical(crate::inputs::regions::UNIT), "regions-index".to_string());
    assert!(Elevation::default().context(&m).contains(&"regions regions-index".to_string()));
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
    let a = super::super::open(d2.path(), &BTreeMap::from([(super::super::logical(UNIT), name.to_string())]), UNIT)
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
        checks: Elevation { store: Some(f.store.path().to_path_buf()), ..Elevation::new(Arc::new(fetch()), None) },
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
    // Dropped files: one added, or one changed, likewise.
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
        .switch(Path::new("/nonexistent"), "sources/inputs/elevation/@index.0123456789abcdef.json")
        .unwrap()
        .expect("switched at once");
    let v: serde_json::Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v, serde_json::json!({ "index": "sources/inputs/elevation/@index.0123456789abcdef.json", "app": APP }));
    // A development build records none: it would bar no app, for good.
    f.checks.app = Some("development".into());
    assert!(f
        .checks
        .switch(Path::new("/nonexistent"), "sources/inputs/elevation/@index.0123456789abcdef.json")
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
    let drop = |p: &str, b: &[u8]| {
        std::fs::write(dir.join(p), b).unwrap();
        std::fs::File::options().write(true).open(dir.join(p)).unwrap().set_modified(old).unwrap();
    };
    let manifest =
        || -> BTreeMap<String, String> { crate::out::read_record(&root.join("state/build/manifest.json")).unwrap() };
    let run = || {
        let mut out = crate::out::Out::unbound(root, &root.join("scratch")).unwrap();
        crate::inputs::gate::run(&mut out, UNIT, false, None).unwrap()
    };
    drop("single.toml", &decl("", "format = \"geotiff\"\nurl = \"https://e.org/a.tif\""));
    let said = run();
    let m = manifest();
    let first = m.get(&super::super::logical(UNIT)).cloned().unwrap_or_else(|| panic!("not taken in: {said}"));
    assert!(switched(&m), "{said}");
    let sw = m[&super::super::switched_logical(UNIT)].clone();
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join(&sw)).unwrap()).unwrap();
    assert_eq!(v, serde_json::json!({ "index": first, "app": APP }));
    // A later version replaces the index; the switch stays as it was.
    drop("second.toml", &decl("rank = 1", "format = \"geotiff\"\nurl = \"https://e.org/t/N48E002.tif\""));
    let said = run();
    let m = manifest();
    assert_ne!(m[&super::super::logical(UNIT)], first, "{said}");
    assert_eq!(m[&super::super::switched_logical(UNIT)], sw);
    TEST_PREPARED.with(|t| *t.borrow_mut() = None);
}
