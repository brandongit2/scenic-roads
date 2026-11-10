use super::*;
use crate::naming::content_name;
use serde_json::json;

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
        {"id": "alps", "name": "Alps", "outline": ["osm:1"], "shapes": {}}]})
    .into();
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

/// The recorded chain: real catalogues (798 to 809 of 30 Oct 2026), trimmed.
fn recorded() -> Vec<Catalogue> {
    crate::testkit::recorded_chain()
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
            c.coverage = json!({"regions": [{"name": "no id"}]}).into();
            c.meta = Value::Null;
            c
        },
        {
            let mut c = sample(9);
            c.coverage = json!({"regions": []}).into();
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
fn each_batch_makes_the_next_catalogue() {
    assert_batches_make_each(&recorded());
}

#[test]
fn incremental_digest_is_the_whole_digest() {
    let (a, mut c) = (sample(7), sample(8));
    c.units.push("6/40/40".into());
    c.files.remove("global/pois");
    c.global.clear();
    let mut cov = c.coverage.to_value();
    cov["regions"].as_array_mut().unwrap().reverse();
    c.coverage = cov.into();
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
    // A batch missing one of its entries (its digest the right map's) doesn't apply either: the
    // digest is of what the map holds after it.
    let mut short = m.batch_to(&entries(&sample(8)).unwrap(), 8, "t", "", "");
    let k = short.set.keys().next().unwrap().clone();
    short.set.remove(&k);
    assert!(m.apply(&short).is_err());
    // In place: applied, or nothing changed; and put back by its undo.
    let mut inplace = m.clone();
    assert!(inplace.apply_in(&d).is_err());
    assert_eq!(inplace, m);
    let u = inplace.apply_in(&m.batch_to(&entries(&sample(8)).unwrap(), 8, "t", "", "")).unwrap();
    assert_eq!(inplace, good);
    inplace.undo(u);
    assert_eq!(inplace, m);
}

#[test]
fn a_batch_of_a_catalogue_is_its_entries_batch() {
    let chain = recorded();
    let mut m = Map::of(&chain[0]).unwrap();
    m.check_is(&chain[0]).unwrap();
    for c in &chain[1..] {
        let b = m.batch_of(c, &c.app, "").unwrap();
        assert_eq!(b, m.batch_to(&entries(c).unwrap(), c.n, &c.created, &c.app, ""));
        assert!(m.check_is(c).is_err(), "catalogue {} changes the map", c.n);
        drop(m.apply_in(&b).unwrap());
        m.check_is(c).unwrap();
    }
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
    let none = BTreeMap::new();
    let mut b = Snapshot::of(&m, &none);
    b.digest = hex(0);
    assert!(Snapshot::decode(&b.encode().unwrap()).is_err());
}

/// The recorded log (`src/map/testdata/log`): the lead's map of 4 Nov 2026 at 1469 (the NAS's
/// snapshot, its entries sampled and its regions' lists cut to three items each) with the lead's
/// cursors and marks, and its batches 1470 to 1484 (their regions cut alike, their digests the
/// sampled map's), all written by the code before #936, whose values were `serde_json::Value`s,
/// with what that code wrote of the map at 1484: its snapshot and its catalogue. Read and written
/// again, each is the bytes it was.
#[test]
fn the_recorded_log_is_written_as_it_was() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/map/testdata/log");
    let d = tempfile::tempdir().unwrap();
    for f in ["snapshot.json.zst", "1470.jsonl"] {
        std::fs::copy(src.join(f), d.path().join(f)).unwrap();
    }
    // Each batch's line (the log's, and what `/map/changes` answers), parsed and written again.
    let seg = std::fs::read(src.join("1470.jsonl")).unwrap();
    let lines: Vec<&[u8]> = seg.split(|c| *c == b'\n').filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 15);
    for l in &lines {
        let b: Batch = serde_json::from_slice(l).unwrap();
        assert!(serde_json::to_vec(&b).unwrap() == *l, "batch {} isn't the bytes it was", b.seq);
    }
    // The snapshot read (its digest checked) and the batches replayed (each one's checked).
    let (at, cursors, marks) = Snapshot::decode_marked(&std::fs::read(src.join("snapshot.json.zst")).unwrap()).unwrap();
    assert_eq!((at.seq, cursors.is_empty(), marks.is_empty()), (1469, false, false));
    for kind in ["f/", "l/", "s/", "u/", "r/", "m/"] {
        assert!(at.entries().keys().any(|k| k.starts_with(kind)), "no {kind} entry");
    }
    let log = log::Log::open(d.path()).unwrap();
    let m = log.map();
    assert_eq!(m.seq, 1484);
    assert!(m.digest_holds());
    // Written: the snapshot and the catalogue the code before wrote of it.
    let snap = Snapshot { marks, ..Snapshot::of(&m, &cursors) }.encode().unwrap();
    assert!(snap == std::fs::read(src.join("expect-1484.json.zst")).unwrap(), "the snapshot isn't the bytes it was");
    let cat = m.catalogue().unwrap();
    let want = zstd::decode_all(&std::fs::read(src.join("expect-1484.catalogue.json.zst")).unwrap()[..]).unwrap();
    assert!(serde_json::to_vec(&cat).unwrap() == want, "the catalogue isn't the bytes it was");
    // And back: the catalogue's entries are the map's.
    assert_eq!(entries(&cat).unwrap(), *m.entries());
}

/// An `f/` entry's content name is its object's `file`, as `v.get("file")` read it of a `Value`:
/// nothing of a list (a derived struct reads `["x"]` as file "x"), a number, or a non-string file.
#[test]
fn a_content_name_is_an_objects_file_only() {
    let of = |v: Value| content_of(&Val::from(v));
    assert_eq!(of(json!({"file": "a.1.pack", "size": 2, "fmt": 1})).as_deref(), Some("a.1.pack"));
    assert_eq!(of(json!({"file": "a\"b"})).as_deref(), Some("a\"b"));
    assert_eq!(of(json!(["a.1.pack"])), None);
    assert_eq!(of(json!([["a.1.pack"]])), None);
    assert_eq!(of(json!({"file": 1})), None);
    assert_eq!(of(json!({"size": 1})), None);
    assert_eq!(of(json!("a.1.pack")), None);
}
