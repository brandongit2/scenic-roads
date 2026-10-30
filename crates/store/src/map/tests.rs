use super::*;
use crate::naming::content_name;
use serde_json::json;
use std::path::{Path, PathBuf};

fn sample(n: u64) -> Catalogue {
    let mut c = Catalogue::new(n);
    c.created = format!("2026-10-30T04:05:{:02}Z", n % 60);
    c.app = "0.1.0".into();
    let mut add = |logical: &str, ext: &str, size: u64| {
        let h = format!("{:016x}", n * 1000 + size);
        c.files.insert(
            logical.into(),
            FileRef { file: content_name(logical, &h, ext), size, fmt: 1, extra: BTreeMap::new() },
        );
        logical.to_string()
    };
    let root = add("layers/roads/root", "pack", 10);
    let lo = add("layers/roads/lo/3-4-2", "pack", 20);
    let hi = add("layers/roads/hi/6-32-21", "pack", 30);
    let deep = add("layers/terrain/deep/10-1-1", "pack", 31);
    let base = add("base/6-32-21", "sect", 40);
    let roads = add("global/roads/6-32-21", "sect", 50);
    let hidata = add("hidata/6-32-21", "sect", 60);
    let pois = add("global/pois", "json", 5);
    let basemap = add("layers/basemap/basemap", "pmtiles", 1000);
    c.units = vec!["6/32/21".into(), "6/32/22".into()];
    c.layers.insert(
        "roads".into(),
        Layer {
            encoding: "rt7".into(),
            minzoom: 4,
            maxzoom: 14,
            root: Some(root),
            lo: [("3/4/2".to_string(), lo)].into(),
            hi: [("6/32/21".to_string(), hi)].into(),
            ..Default::default()
        },
    );
    c.layers.insert(
        "terrain".into(),
        Layer {
            encoding: "webp".into(),
            deep: [("10/1/1".to_string(), deep)].into(),
            native_zoom: [("10/1/1".to_string(), 14)].into(),
            extra: [("tilesize".to_string(), json!(256))].into(),
            ..Default::default()
        },
    );
    c.basemap = vec![basemap];
    c.base.insert("6/32/21".into(), base);
    c.roads.insert("6/32/21".into(), roads);
    c.hidata.insert("6/32/21".into(), hidata);
    c.global.insert("pois.json".into(), pois);
    c.meta = json!({"bounds": [1, 2, 3, 4]});
    c.credits = json!([{"what": "Roads", "source": "OpenStreetMap", "terms": "ODbL"}]);
    c.dem_labels = [(1u8, "SRTM".to_string())].into();
    c.coverage = json!({"recorded": true, "regions": [
        {"id": "northumberland", "name": "Northumberland", "outline": ["osm:88066"], "shapes": {}},
        {"id": "alps", "name": "Alps", "outline": ["osm:1"], "shapes": {}}]});
    c.extra.insert("future".into(), json!({"x": 1}));
    c
}

/// Entries → `Catalogue` → bytes: identical to the catalogue's own.
fn assert_round_trips(c: &Catalogue) {
    let m = Map::of(c).unwrap();
    let back = m.catalogue().unwrap();
    assert_eq!(&back, c, "catalogue {}", c.n);
    assert_eq!(serde_json::to_vec(&back).unwrap(), serde_json::to_vec(c).unwrap(), "catalogue {}", c.n);
    assert_eq!(entries(&back).unwrap(), *m.entries());
}

/// The catalogues of a folder, read whole, lowest first.
fn chain_in(dir: &Path) -> Vec<Catalogue> {
    let mut ns = crate::catalogue::list(dir).unwrap();
    ns.reverse();
    ns.into_iter().map(|n| crate::catalogue::read_n(dir, n).unwrap()).collect()
}

/// The recorded chain: real catalogues (798 to 809 of 30 Oct 2026), trimmed (`record_chain`).
fn recorded() -> Vec<Catalogue> {
    chain_in(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/map/testdata"))
}

#[test]
fn every_catalogue_round_trips() {
    for c in [
        sample(7),
        Catalogue::new(1),
        {
            // (Odd but readable: units out of order, a coverage that isn't split, a null meta.)
            let mut c = sample(8);
            c.units = vec!["6/9/9".into(), "6/1/1".into()];
            c.coverage = json!({"regions": [{"name": "no id"}]});
            c.meta = Value::Null;
            c
        },
        {
            let mut c = sample(9);
            c.coverage = json!({"regions": []});
            c
        },
    ] {
        assert_round_trips(&c);
    }
    let chain = recorded();
    assert!(chain.len() >= 10, "the recorded chain is there");
    for c in &chain {
        assert_round_trips(c);
    }
}

/// When `SCENIC_MAP_CHAIN` names a folder of catalogues (a copy of the NAS's `catalog/`), every
/// one there round trips, and each batch between two gives the next.
#[test]
fn a_real_chain_round_trips() {
    let Some(dir) = std::env::var_os("SCENIC_MAP_CHAIN") else { return };
    let chain = chain_in(Path::new(&dir));
    for c in &chain {
        assert_round_trips(c);
    }
    assert_batches_make_each(&chain);
    eprintln!("{} catalogues round trip", chain.len());
}

fn assert_batches_make_each(chain: &[Catalogue]) {
    let mut m = Map::of(&chain[0]).unwrap();
    for c in &chain[1..] {
        let b = m.batch_to(&entries(c).unwrap(), c.n, &c.created, &c.app, "");
        // (Through its JSON line, as the log keeps it.)
        let b: Batch = serde_json::from_slice(&serde_json::to_vec(&b).unwrap()).unwrap();
        m = m.apply(&b).unwrap();
        assert_eq!(&m.catalogue().unwrap(), c, "catalogue {} by its batch", c.n);
        assert_eq!(m.digest(), digest_of(&entries(c).unwrap()), "incremental digest of {}", c.n);
    }
}

#[test]
fn batch_between_equals_the_delta() {
    let chain = recorded();
    assert_batches_make_each(&chain);
    // (And the deltas the catalogues were stored as give the same.)
    for w in chain.windows(2) {
        let d = crate::catalogue::Delta::between(&w[0], &w[1]).unwrap();
        let m = Map::of(&w[0]).unwrap();
        let b = m.batch_to(&entries(&w[1]).unwrap(), w[1].n, &w[1].created, "", "");
        assert_eq!(m.apply(&b).unwrap().catalogue().unwrap(), d.apply(&w[0]).unwrap());
    }
}

#[test]
fn incremental_digest_is_the_whole_digest() {
    let (a, mut c) = (sample(7), sample(8));
    c.units.push("6/40/40".into());
    c.files.remove("global/pois");
    c.global.clear();
    c.coverage["regions"].as_array_mut().unwrap().reverse();
    let m = Map::of(&a).unwrap();
    let b = m.batch_to(&entries(&c).unwrap(), 8, &c.created, "", "");
    assert_eq!(b.digest, hex(digest_of(&entries(&c).unwrap())));
    let after = m.apply(&b).unwrap();
    assert!(after.digest_holds());
    assert_eq!(after.catalogue().unwrap(), c);
    // `freed`: each content name sample 7 named and 8 doesn't.
    let mut gone: Vec<String> = a.files.values().map(|f| f.file.clone()).collect();
    gone.sort();
    assert_eq!(b.freed, gone, "every file is renamed or dropped");
    assert!(b.del.contains(&"f/global/pois".to_string()) && b.del.contains(&"s/global/pois.json".to_string()));
    // Nothing changed: an empty batch.
    assert!(after.batch_to(&entries(&c).unwrap(), 9, "", "", "").is_empty());
}

#[test]
fn a_batch_is_checked_by_its_digest() {
    let a = sample(7);
    let m = Map::of(&a).unwrap();
    let mut b = m.batch_to(&entries(&sample(8)).unwrap(), 8, "t", "", "");
    let good = m.apply(&b).unwrap();
    assert_eq!(good.seq, 8);
    // A wrong digest, an entry deleted that isn't there, a seq not past the map's: refused, and the
    // map it was applied to is as it was.
    b.digest = hex(digest_of(&entries(&sample(9)).unwrap()));
    assert!(m.apply(&b).is_err());
    let mut d = m.batch_to(&entries(&sample(8)).unwrap(), 8, "t", "", "");
    d.del.push("u/6/0/0".into());
    assert!(m.apply(&d).is_err());
    assert!(good.apply(&m.batch_to(&entries(&sample(8)).unwrap(), 8, "t", "", "")).is_err());
    assert_eq!(m, Map::of(&a).unwrap());
    // Several applied to one copy.
    let b9 = good.batch_to(&entries(&sample(9)).unwrap(), 9, "t9", "", "");
    let b8 = m.batch_to(&entries(&sample(8)).unwrap(), 8, "t", "", "");
    assert_eq!(m.apply_all(&[b8, b9]).unwrap().catalogue().unwrap(), Catalogue { created: "t9".into(), ..sample(9) });
}

#[test]
fn a_snapshot_reads_back_and_the_nas_keeps_three() {
    let d = tempfile::tempdir().unwrap();
    let root = NasRoot::local_for_test(d.path());
    for n in 1..=5 {
        write_nas_snapshot(&root, &Map::of(&sample(n)).unwrap()).unwrap();
    }
    let mut names: Vec<String> = std::fs::read_dir(d.path().join(NAS_DIR))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["3.json.zst", "4.json.zst", "5.json.zst"]);
    let (m, cursors) = Snapshot::decode(&std::fs::read(d.path().join("map/5.json.zst")).unwrap()).unwrap();
    assert_eq!(m.catalogue().unwrap(), sample(5));
    assert!(cursors.is_empty());
    // A damaged one doesn't read.
    let mut b = Snapshot::of(&m, BTreeMap::new());
    b.digest = hex(0);
    assert!(Snapshot::decode(&b.encode().unwrap()).is_err());
}

/// Makes the recorded chain (`src/map/testdata/`) from a folder of real catalogues named by
/// `SCENIC_MAP_RECORD` (a copy of the NAS's `catalog/`): each trimmed to the files its chain
/// changes and one in 200 of the rest, the sections naming them, two regions besides those that
/// change, the regions' shapes emptied. Nothing without it.
#[test]
fn record_chain() {
    let Some(dir) = std::env::var_os("SCENIC_MAP_RECORD") else { return };
    let dir = PathBuf::from(dir);
    let chain = chain_in(&dir);
    let mut changed: std::collections::BTreeSet<String> = Default::default();
    for w in chain.windows(2) {
        let (a, b) = (entries(&w[0]).unwrap(), entries(&w[1]).unwrap());
        for k in a.keys().chain(b.keys()) {
            if a.get(k) != b.get(k) {
                changed.insert(k.clone());
            }
        }
    }
    let keep_file =
        |l: &str| changed.contains(&format!("f/{l}")) || xxhash_rust::xxh3::xxh3_64(l.as_bytes()) % 200 == 0;
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/map/testdata");
    let _ = std::fs::remove_dir_all(&out);
    for c in &chain {
        let mut t = c.clone();
        t.files.retain(|l, _| keep_file(l));
        let kept = |v: &String| t.files.contains_key(v);
        let files = t.files.clone();
        let kept2 = |v: &String| files.contains_key(v);
        for l in t.layers.values_mut() {
            for m in [&mut l.lo, &mut l.hi, &mut l.deep, &mut l.polar] {
                m.retain(|_, v| kept2(v));
            }
            let deep = l.deep.clone();
            l.native_zoom.retain(|k, _| deep.contains_key(k));
            l.root = l.root.take().filter(|r| kept2(r));
        }
        t.layers.retain(|_, l| !l.lo.is_empty() || !l.hi.is_empty() || !l.deep.is_empty() || l.root.is_some());
        for m in [&mut t.base, &mut t.roads, &mut t.hidata, &mut t.markdata, &mut t.ovdata, &mut t.global] {
            m.retain(|_, v| kept(v));
        }
        let units: std::collections::BTreeSet<String> = t.base.keys().cloned().collect();
        t.units.retain(|u| units.contains(u) || changed.contains(&format!("u/{u}")));
        t.basemap.retain(|b| kept(b));
        if let Some(rs) = t.coverage.get_mut("regions").and_then(Value::as_array_mut) {
            let mut i = 0;
            rs.retain(|r| {
                i += 1;
                let id = r.get("id").and_then(Value::as_str).unwrap_or("");
                i <= 2 || changed.contains(&format!("r/{id}"))
            });
            for r in rs.iter_mut() {
                if let Some(o) = r.as_object_mut() {
                    o.insert("shapes".into(), json!({}));
                }
            }
        }
        crate::catalogue::write_copy(&out, &t).unwrap();
    }
}
