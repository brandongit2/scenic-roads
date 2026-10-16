//! The elevation declarations' checks, over versions held in memory and data served by a map of
//! URLs (no network): shapes, references, the data measured, and the plan a unit reads.

use super::*;
use crate::fetch::MapFetch;

const PX: f64 = 1.0 / 3600.0;

/// A 1° GeoTIFF at 1″ (EPSG:4326) with its south-west corner at `lon`, `lat`, 36 × 36 pixels of
/// 100″ for speed (so 100″, about 3 km).
fn tile(lon: f64, lat: f64) -> Vec<u8> {
    let data = vec![12.5f32; 36 * 36];
    crate::geotiff::write_f32(36, 36, &data, 16, [lon, 100.0 * PX, 0.0, lat + 1.0, 0.0, -100.0 * PX], 4326, Some(-9999.0)).unwrap()
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
}

impl Fixture {
    fn new(files: Vec<(&str, Vec<u8>)>) -> Fixture {
        let mut f = MapFetch::default();
        f.0.insert("https://e.org/a.tif".into(), Some(tile(2.0, 48.0)));
        f.0.insert("https://e.org/t/N48E002.tif".into(), Some(tile(2.0, 48.0)));
        f.0.insert("https://e.org/t/N49E002.tif".into(), Some(tile(2.0, 49.0)));
        f.0.insert("https://e.org/gone.tif".into(), None);
        Fixture { files: files.into_iter().map(|(p, b)| (p.to_string(), Arc::new(b))).collect(), checks: Elevation::new(Arc::new(f), None) }
    }

    fn whole(&self) -> Vec<Finding> {
        let rd = |p: &str| self.files.get(p).cloned().ok_or_else(|| anyhow::anyhow!("{p}: none"));
        self.checks.whole(&Version { paths: self.files.keys().cloned().collect(), read: &rd }).unwrap()
    }

    fn derive(&self) -> Derived {
        let rd = |p: &str| self.files.get(p).cloned().ok_or_else(|| anyhow::anyhow!("{p}: none"));
        self.checks.derive(&Version { paths: self.files.keys().cloned().collect(), read: &rd }, &BTreeSet::new()).unwrap()
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
fn good_declarations_are_measured_from_their_data() {
    let f = Fixture::new(vec![
        ("single.toml", decl("", "format = \"geotiff\"\nurl = \"https://e.org/a.tif\"")),
        ("tiled.toml", decl("rank = 1", "format = \"geotiff\"\nindex = \"tiled.geojson\"\nstore = true")),
        ("tiled.geojson", index(&[("https://e.org/t/N48E002.tif", [2.0, 48.0, 3.0, 49.0]), ("https://e.org/t/N49E002.tif", [2.0, 49.0, 3.0, 50.0])])),
        ("pyramid.toml", decl("territory = \"world\"\nrank = 2", "format = \"tiles\"\nurl = \"https://e.org/{z}/{x}/{y}.png\"\nzoom = 12\nencoding = \"terrarium\"")),
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
        files.insert(p.clone(), super::super::FileEntry { file: copy, size: b.len() as u64, keyed: c.keyed.unwrap_or_else(|| "-".into()), facts });
    }
    let ix = super::super::Index { fmt: 1, unit: UNIT.into(), checks: "elevation 1".into(), files, accepted: BTreeMap::new() };
    let name = "sources/inputs/elevation/@index.0000000000000000.json";
    std::fs::write(d2.path().join(name), serde_json::to_vec(&ix).unwrap()).unwrap();
    let a = super::super::open(d2.path(), &BTreeMap::from([(super::super::logical(UNIT), name.to_string())]), UNIT).unwrap().unwrap();
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
        ("ngvd.toml", String::from_utf8(decl("", "format = \"geotiff\"\nurl = \"https://e.org/a.tif\"")).unwrap().replace("EGM2008", "NGVD29").into_bytes()),
    ]);
    let got = f.checks();
    assert_eq!(got, vec![DATA, DECL, MISSING, ORPHAN, ORPHAN, SHAPE, TIE], "{:?}", f.whole());
    // The tie names both; ids are stable.
    let tie = f.whole().into_iter().find(|x| x.check() == TIE).unwrap();
    assert_eq!(tie.files, vec!["bad.toml".to_string(), "twin.toml".to_string()]);
    assert_eq!(tie.id, f.whole().into_iter().find(|x| x.check() == TIE).unwrap().id);
}

#[test]
fn the_switch_is_the_proofs_of_exactly_the_version_named() {
    let d = tempfile::tempdir().unwrap();
    let e = Elevation::default();
    assert_eq!(e.switch(d.path(), "v1").unwrap(), None);
    std::fs::create_dir_all(d.path().join("state/inputs")).unwrap();
    std::fs::write(d.path().join(PROOF), br#"{"index": "v1", "ok": true, "app": "20261016-0000-aaaaaaa"}"#).unwrap();
    assert!(e.switch(d.path(), "v1").unwrap().is_some());
    assert_eq!(e.switch(d.path(), "v2").unwrap(), None);
    std::fs::write(d.path().join(PROOF), br#"{"index": "v1", "ok": false, "app": "20261016-0000-aaaaaaa"}"#).unwrap();
    assert_eq!(e.switch(d.path(), "v1").unwrap(), None);
}
