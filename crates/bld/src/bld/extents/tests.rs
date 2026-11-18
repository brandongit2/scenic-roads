use super::*;
use crate::bld::work::{osm, Block, ZSTD_LEVEL};
use std::path::Path;
use store::nasfs::NasRoot;

fn out(root: &Path) -> Out {
    Out::unbound(&NasRoot::local_for_test(root), &root.join("scratch")).unwrap()
}

/// A building of a block: its height (dm), floors, flags, dataset code and OSM id.
type Rec = (u16, u8, u8, u8, u64);

/// A block of `recs`, each a triangle.
fn block(recs: &[Rec]) -> Vec<u8> {
    let n = recs.len();
    let mut b = Block {
        cen: vec![[0, 0]; n],
        area: vec![100.0; n],
        npolys: vec![1; n],
        nrings: vec![1; n],
        ring_len: vec![3; n],
        verts: (0..n).flat_map(|_| [[0, 0], [10, 0], [0, 10]]).collect(),
        h: recs.iter().map(|r| r.0).collect(),
        m: vec![0; n],
        f: recs.iter().map(|r| r.1).collect(),
        mf: vec![0; n],
        class: vec![0; n],
        subtype: vec![0; n],
        roof: vec![0; n],
        flags: recs.iter().map(|r| r.2).collect(),
        hsrc: recs.iter().map(|r| r.3).collect(),
        ghsl: vec![0; n],
        osm: recs.iter().map(|r| r.4).collect(),
        ..Default::default()
    };
    b.index();
    zstd::bulk::compress(&b.encode(), ZSTD_LEVEL).unwrap()
}

/// Z6 tile (x, y)'s normalized file holding `blocks` ((column, row) of its z14 cells, records), its
/// datasets `srcs`, put in the records under its logical name.
fn put(o: &mut Out, dir: &Path, (x, y): (u32, u32), srcs: &[&str], blocks: &[((u32, u32), Vec<Rec>)]) {
    let meta = Meta {
        fmt: 1,
        tile: format!("6/{x}/{y}"),
        srcs: srcs.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    };
    let mut bs: Vec<(u64, Vec<u8>, u32)> = blocks
        .iter()
        .map(|((c, r), recs)| {
            (roadcore::archive::tile_key(14, x * CELLS + c, y * CELLS + r), block(recs), recs.len() as u32)
        })
        .collect();
    bs.sort_by_key(|b| b.0);
    let p = dir.join(format!("w-{x}-{y}.sect"));
    crate::bld::work::write(&p, &meta, &mut bs.into_iter().map(Ok)).unwrap();
    o.put_file(&crate::bld::work_logical(x, y), "sect", &p).unwrap();
}

fn spec(id: &str, dataset: &str, holds: &str, share: f64) -> Spec {
    Spec { id: format!("overture/{id}"), dataset: dataset.into(), holds: holds.into(), share }
}

fn specs4() -> Vec<Spec> {
    vec![
        spec("osm", OSM, "footprints", 0.0),
        spec("osm-heights", OSM, "heights", 0.5),
        spec("osm-storeys", OSM, "storeys", 0.5),
        spec("ms", "Microsoft ML Buildings", "footprints", 0.0),
    ]
}

/// The cells (column, row) a mask holds in z6 tile t.
fn cells(m: &Mask, t: (u32, u32)) -> Vec<(u16, u16)> {
    let Some(c) = m.tiles.get(&t) else { return Vec::new() };
    (0..CELLS as usize)
        .flat_map(|r| c.row(r).iter().flat_map(move |&(a, b)| (a..b).map(move |x| (x, r as u16))))
        .collect()
}

#[test]
fn a_cell_holds_a_piece_by_its_buildings() {
    let d = tempfile::tempdir().unwrap();
    let mut o = out(d.path());
    let ms = "Microsoft ML Buildings";
    // (Codes: 1 Esri, 2 Microsoft, 3 OSM.)
    let srcs = ["Esri Community Maps", ms, OSM];
    let way = osm::WAY | 7;
    put(
        &mut o,
        d.path(),
        (33, 22),
        &srcs,
        &[
            // OSM's footprints with OSM's heights on two of three: heights held (2/3 ≥ 0.5).
            ((5, 1), vec![(120, 0, 0, 3, way), (90, 0, 0, 3, way), (0, 0, 0, 3, way)]),
            // One OSM height of three, two Microsoft footprints: OSM's footprints, not its heights.
            ((6, 1), vec![(120, 0, 0, 3, way), (0, 0, 0, 2, 0), (0, 0, 0, 2, 0)]),
            // OSM's storeys on both (no height); a part not counted.
            ((7, 1), vec![(0, 4, 0, 3, way), (0, 2, 0, 3, way), (0, 0, flag::PART, 3, way)]),
            // An OSM footprint whose height is Esri's: OSM's footprint, Esri's height (undeclared).
            ((200, 9), vec![(150, 0, 0, 1, way)]),
            // Only parts: no buildings.
            ((201, 9), vec![(0, 0, flag::PART, 3, way)]),
        ],
    );
    let s = specs4();
    let m = masks(&mut o, "buildings", &s, &|_, _| {}).unwrap();
    assert_eq!((m.read, m.kept), (1, 0));
    let t = (33, 22);
    assert_eq!(cells(&m.masks[0], t), [(5, 1), (6, 1), (7, 1), (200, 9)]);
    assert_eq!(cells(&m.masks[1], t), [(5, 1)]);
    assert_eq!(cells(&m.masks[2], t), [(7, 1)]);
    assert_eq!(cells(&m.masks[3], t), [(6, 1)]);
    // Side by side, one run.
    assert_eq!(m.masks[0].tiles[&t].row(1), [(5, 8)]);
    assert_eq!(m.undeclared.get(&("heights".into(), "Esri Community Maps".into())), Some(&1));
    assert_eq!(m.undeclared.get(&("storeys".into(), OSM.into())), None);
    // Again: the record's cells, nothing read, the same masks and the same record.
    let rec = o.get(&codecs::available::masks_logical("buildings")).unwrap().to_string();
    let m2 = masks(&mut o, "buildings", &s, &|_, _| {}).unwrap();
    assert_eq!((m2.read, m2.kept), (0, 1));
    for (a, b) in m.masks.iter().zip(&m2.masks) {
        assert_eq!((&a.id, &a.tiles), (&b.id, &b.tiles));
    }
    assert_eq!(o.get(&codecs::available::masks_logical("buildings")).unwrap(), rec);
    // A second file: it alone read; its masks beside the first's.
    put(&mut o, d.path(), (34, 22), &srcs, &[((0, 0), vec![(0, 0, 0, 2, 0)])]);
    let m3 = masks(&mut o, "buildings", &s, &|_, _| {}).unwrap();
    assert_eq!((m3.read, m3.kept), (1, 1));
    assert_eq!(cells(&m3.masks[3], (34, 22)), [(0, 0)]);
    assert_eq!(cells(&m3.masks[3], t), [(6, 1)]);
    // A share changed: every file read again; a piece gone from a file's cells.
    let mut s2 = specs4();
    s2[1].share = 0.9;
    let m4 = masks(&mut o, "buildings", &s2, &|_, _| {}).unwrap();
    assert_eq!((m4.read, m4.kept), (2, 0));
    assert!(cells(&m4.masks[1], t).is_empty() && !m4.masks[1].tiles.contains_key(&t));
    // A file gone: its tile gone from the masks and the record.
    o.remove(&crate::bld::work_logical(34, 22));
    let m5 = masks(&mut o, "buildings", &s2, &|_, _| {}).unwrap();
    assert_eq!((m5.read, m5.kept), (0, 1));
    assert!(!m5.masks[3].tiles.contains_key(&(34, 22)));
    // No mask piece: nothing read, the record gone.
    let m6 = masks(&mut o, "buildings", &[], &|_, _| {}).unwrap();
    assert!(m6.masks.is_empty() && m6.read == 0 && m6.kept == 0);
    assert!(o.get(&codecs::available::masks_logical("buildings")).is_none());
}

#[test]
fn a_record_unreadable_or_of_other_specs_is_read_again() {
    let d = tempfile::tempdir().unwrap();
    let mut o = out(d.path());
    put(&mut o, d.path(), (10, 20), &[OSM], &[((3, 4), vec![(0, 0, 0, 1, osm::WAY | 1)])]);
    let s = specs4();
    masks(&mut o, "buildings", &s, &|_, _| {}).unwrap();
    let l = codecs::available::masks_logical("buildings");
    let b = o.path(o.get(&l).unwrap()).read().unwrap();
    // Its own bytes decode to what it holds; other specs' digest holds nothing; cut short, refused.
    let digest = specs_digest(&s);
    assert_eq!(decode(&b, &digest, s.len()).unwrap().len(), 1);
    assert!(decode(&b, &specs_digest(&s[..2]), 2).unwrap().is_empty());
    assert!(decode(&b[..b.len() - 3], &digest, s.len()).is_err());
    o.put_bytes(&l, "bin", b"not a record").unwrap();
    let m = masks(&mut o, "buildings", &s, &|_, _| {}).unwrap();
    assert_eq!((m.read, m.kept), (1, 0));
    assert_eq!(cells(&m.masks[0], (10, 20)), [(3, 4)]);
}

#[test]
fn specs_come_from_the_overture_blocks_readings() {
    let pieces = br#"{"type": "FeatureCollection", "source": "overture", "theme": "buildings",
        "read": [{"piece": "osm-heights", "method": "overture", "dataset": "OpenStreetMap", "holds": "heights",
        "share": 0.5, "grain": "z14 tiles"}, {"piece": "city", "method": "file"}],
        "features": [{"type": "Feature", "geometry": null, "properties": {"piece": "osm-heights", "extent": "mask",
        "availability": "open", "detail": "heights"}}, {"type": "Feature", "geometry": null, "properties":
        {"piece": "city", "extent": "unknown", "availability": "open", "detail": "lod2"}}]}"#;
    let s = specs("overture", pieces).unwrap();
    assert_eq!(s, [spec("osm-heights", OSM, "heights", 0.5)]);
    // An overture block's piece of another class, or a file that isn't one of pieces: refused.
    let square = r#"{"type": "MultiPolygon", "coordinates": [[[[0, 0], [1, 0], [1, 1], [0, 1], [0, 0]]]]}"#;
    let other = String::from_utf8(pieces.to_vec())
        .unwrap()
        .replacen(r#""geometry": null"#, &format!(r#""geometry": {square}"#), 1)
        .replacen("\"mask\"", "\"index\"", 1);
    assert!(specs("overture", other.as_bytes()).is_err());
    assert!(specs("overture", b"[]").is_err());
    assert_eq!(row_runs(vec![9, 3, 4, 4, 6, 5]), [(3, 7), (9, 10)]);
}

#[test]
fn a_files_blocks_are_read_in_runs() {
    let e = |offset: u64, len: u32| IndexEntry { key: offset, offset, len, count: 1 };
    let big = READ_RUN as u32;
    let ix = [e(0, 10), e(10, 20), e(30, big), e(30 + u64::from(big), 5)];
    let r = runs(&ix);
    assert_eq!(r.iter().map(|r| r.len()).collect::<Vec<_>>(), [2, 1, 1]);
    assert!(runs(&[]).is_empty());
}

/// The buildings' step on the real data, a measure run by hand: the declarations of
/// producers/extents/buildings/, the normalized files the records of the NAS root
/// `$SCENIC_MASKS_ROOT` name (read there, through links in a local folder: nothing is written to
/// the NAS), cut to the land of the regions-land file `$SCENIC_RENDER_LAND`. It prints the masks'
/// first run (every file read), a second (the record holding them all), and the layers' build, each
/// timed; the peak is the process's (`/usr/bin/time -l`). Nothing without the variables; on the
/// Mac mini, near the NAS (tools/dev/agent-rules.md).
#[test]
fn measures_the_real_step() {
    let (Some(nas), Some(land)) = (std::env::var_os("SCENIC_MASKS_ROOT"), std::env::var_os("SCENIC_RENDER_LAND"))
    else {
        return;
    };
    let nas = std::path::PathBuf::from(nas);
    let brief: serde_json::Value =
        serde_json::from_slice(&std::fs::read(nas.join("state/build/brief.json")).unwrap()).unwrap();
    let manifest: BTreeMap<String, String> = serde_json::from_value(brief["manifest"].clone()).unwrap();
    let d = tempfile::tempdir().unwrap();
    let mut o = out(d.path());
    for (l, c) in manifest.range(crate::bld::WORK.to_string()..).take_while(|(l, _)| l.starts_with(crate::bld::WORK)) {
        let link = d.path().join(c);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(nas.join(c), &link).unwrap();
        o.manifest.insert(l.clone(), c.clone());
    }
    // (`$SCENIC_MASKS_DECLS`: the declarations' folder, for a test binary run away from its tree.)
    let src = std::env::var_os("SCENIC_MASKS_DECLS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../producers/extents/buildings"));
    let (mut pieces, mut specs_all) = (Vec::new(), Vec::new());
    for e in std::fs::read_dir(&src).unwrap() {
        let p = e.unwrap().path();
        let Some(id) = p.file_name().unwrap().to_str().unwrap().strip_suffix(".geojson").map(str::to_string) else {
            continue;
        };
        let geojson = std::fs::read(&p).unwrap();
        pieces.extend(terrain::available::pieces(&id, &geojson).unwrap());
        specs_all.extend(specs(&id, &geojson).unwrap());
    }
    let files = o.manifest.len();
    let t = std::time::Instant::now();
    let m = masks(&mut o, "buildings", &specs_all, &|_, _| {}).unwrap();
    eprintln!("masks: {} files ({} read, {} kept) in {:.1?}", files, m.read, m.kept, t.elapsed());
    for k in &m.masks {
        eprintln!("  {}: {} z6 tiles", k.id, k.tiles.len());
    }
    for ((holds, dataset), n) in &m.undeclared {
        eprintln!("  undeclared: {n} buildings' {holds} of {dataset}");
    }
    let t = std::time::Instant::now();
    let again = masks(&mut o, "buildings", &specs_all, &|_, _| {}).unwrap();
    eprintln!("masks again: {} read, {} kept in {:.1?}", again.read, again.kept, t.elapsed());
    let record = o.path(o.get(&codecs::available::masks_logical("buildings")).unwrap()).read().unwrap().len();
    eprintln!("the record: {record} bytes");
    let t = std::time::Instant::now();
    let land = inputs::inputs::regions::land::Land::open(std::fs::File::open(land).unwrap()).unwrap();
    let cut = terrain::available::Cut::of_theme(&land, "buildings", Box::new(|_: &str| Ok(None))).unwrap();
    let made = terrain::available::build(&mut o, "buildings", &pieces, &m.masks, &cut, &|_, _| {}).unwrap();
    o.save().unwrap();
    let bytes: u64 = o
        .manifest
        .iter()
        .filter(|(l, _)| l.starts_with("layers/available-buildings/"))
        .map(|(_, c)| std::fs::metadata(d.path().join(c)).unwrap().len())
        .sum();
    eprintln!(
        "build: {} pieces and {} masks in {:.1?}: {} packs, {} tiles, {bytes} bytes of packs",
        pieces.len(),
        m.masks.len(),
        t.elapsed(),
        made.packs,
        made.tiles
    );
}
