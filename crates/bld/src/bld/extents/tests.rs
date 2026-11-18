use super::*;
use crate::bld::work::{Block, ZSTD_LEVEL};
use std::path::Path;
use store::nasfs::NasRoot;

fn out(root: &Path) -> Out {
    Out::unbound(&NasRoot::local_for_test(root), &root.join("scratch")).unwrap()
}

/// A building of a block: its height (dm), floors, flags, height's dataset code and footprint's.
type Rec = (u16, u8, u8, u8, u8);

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
        osm: vec![0; n],
        ..Default::default()
    };
    b.index();
    zstd::bulk::compress(&b.encode(), ZSTD_LEVEL).unwrap()
}

/// Z6 tile (x, y)'s normalized file holding `blocks` ((column, row) of its z14 cells, records),
/// its datasets `srcs`, put in the records under its logical name; its content name, and its
/// records' footprints' datasets in its order (the side file's column).
fn put_work(
    o: &mut Out,
    dir: &Path,
    (x, y): (u32, u32),
    srcs: &[&str],
    blocks: &[((u32, u32), Vec<Rec>)],
) -> (String, Vec<u8>) {
    let meta = Meta {
        fmt: 1,
        tile: format!("6/{x}/{y}"),
        srcs: srcs.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    };
    let mut bs: Vec<(u64, &Vec<Rec>)> = blocks
        .iter()
        .map(|((c, r), recs)| (roadcore::archive::tile_key(14, x * CELLS + c, y * CELLS + r), recs))
        .collect();
    bs.sort_by_key(|b| b.0);
    let fsrc = bs.iter().flat_map(|(_, recs)| recs.iter().map(|r| r.4)).collect();
    let p = dir.join(format!("w-{x}-{y}.sect"));
    crate::bld::work::write(&p, &meta, &mut bs.iter().map(|(k, recs)| Ok((*k, block(recs), recs.len() as u32))))
        .unwrap();
    (o.put_file(&crate::bld::work_logical(x, y), "sect", &p).unwrap(), fsrc)
}

/// Z6 tile (x, y)'s side file, naming the normalized file `work`, put under its logical name.
fn put_side(o: &mut Out, dir: &Path, (x, y): (u32, u32), work: &str, fsrc: &[u8]) {
    let p = dir.join(format!("s-{x}-{y}.sect"));
    crate::bld::side::write(&p, &format!("6/{x}/{y}"), work, fsrc).unwrap();
    o.put_file(&crate::bld::side_logical(x, y), "sect", &p).unwrap();
}

/// Z6 tile t's normalized file and its side file.
fn put(o: &mut Out, dir: &Path, t: (u32, u32), srcs: &[&str], blocks: &[((u32, u32), Vec<Rec>)]) {
    let (work, fsrc) = put_work(o, dir, t, srcs, blocks);
    put_side(o, dir, t, &work, &fsrc);
}

const OSM: &str = "OpenStreetMap";
const MS: &str = "Microsoft ML Buildings";
const ESRI: &str = "Esri Community Maps";

fn spec(id: &str, dataset: &str, holds: &str) -> Spec {
    Spec { id: format!("overture/{id}"), block: MaskBlock { dataset: dataset.into(), holds: holds.into() } }
}

fn specs5() -> Vec<Spec> {
    vec![
        spec("osm", OSM, "footprints"),
        spec("osm-heights", OSM, "heights"),
        spec("osm-storeys", OSM, "storeys"),
        spec("ms", MS, "footprints"),
        spec("esri-heights", ESRI, "heights"),
    ]
}

/// The cells (column, row) a mask holds in z6 tile t.
fn cells(m: &Mask, t: (u32, u32)) -> Vec<(u16, u16)> {
    let Some(c) = m.tiles.get(&t) else { return Vec::new() };
    (0..CELLS as usize)
        .flat_map(|r| c.row(r).iter().flat_map(move |&(a, b)| (a..b).map(move |x| (x, r as u16))))
        .collect()
}

/// The cells' rule: footprints the plurality's (ties to the first code; code 0, no dataset, first
/// of all), never a stray building's; heights where half the buildings have one, each dataset of a
/// quarter of them; storeys where half the buildings with no height have floors.
#[test]
fn the_cells_rule() {
    let fp = |n: &[u32]| n.iter().map(|&k| [k, 0, 0]).collect::<Vec<_>>();
    // (Code 0, no dataset: counted, never credited.)
    assert_eq!(credit(&fp(&[0, 1, 10]), 11, FOOTPRINTS), [2]);
    assert_eq!(credit(&fp(&[0, 3, 3]), 6, FOOTPRINTS), [1]);
    assert_eq!(credit(&fp(&[5, 3, 1]), 9, FOOTPRINTS), Vec::<u8>::new());
    // (A tie with code 0: no dataset.)
    assert_eq!(credit(&fp(&[3, 3, 1]), 7, FOOTPRINTS), Vec::<u8>::new());
    let ht = |n: &[u32]| n.iter().map(|&k| [0, 0, k]).collect::<Vec<_>>();
    // Houston: two datasets' heights on 44 % of the buildings each: both.
    assert_eq!(credit(&ht(&[0, 44, 44]), 100, HEIGHTS), [1, 2]);
    // Under half the buildings with heights: none; a dataset under a quarter of them: not it.
    assert_eq!(credit(&ht(&[0, 30, 5]), 100, HEIGHTS), Vec::<u8>::new());
    assert_eq!(credit(&ht(&[0, 50, 10]), 100, HEIGHTS), [1]);
    assert_eq!(credit(&ht(&[0, 0, 0]), 0, HEIGHTS), Vec::<u8>::new());
    let st = |n: &[u32]| n.iter().map(|&k| [0, k, 0]).collect::<Vec<_>>();
    assert_eq!(credit(&st(&[0, 3, 2]), 6, STOREYS), [1, 2]);
    // 60 % heights and 40 % storeys alone: the storeys are all the buildings with no height.
    let both = |hs: u32, st: u32| vec![[0, 0, 0], [0, st, hs]];
    assert_eq!(credit(&both(60, 40), 100, STOREYS), [1]);
    assert_eq!(credit(&both(60, 40), 100, HEIGHTS), [1]);
    // Storeys on under half the buildings with no height: none.
    assert_eq!(credit(&both(60, 19), 100, STOREYS), Vec::<u8>::new());
    // HOLDS's order, as these indexes take it.
    assert_eq!([HOLDS[FOOTPRINTS], HOLDS[STOREYS], HOLDS[HEIGHTS]], ["footprints", "storeys", "heights"]);
}

#[test]
fn a_cell_holds_a_piece_by_its_buildings() {
    let d = tempfile::tempdir().unwrap();
    let mut o = out(d.path());
    // (Codes: 1 Esri, 2 Microsoft, 3 OSM.)
    let srcs = [ESRI, MS, OSM];
    put(
        &mut o,
        d.path(),
        (33, 22),
        &srcs,
        &[
            // OSM's footprints with OSM's heights on two of three: heights held (2/3 ≥ 1/2).
            ((5, 1), vec![(120, 0, 0, 3, 3), (90, 0, 0, 3, 3), (0, 0, 0, 3, 3)]),
            // One OSM building with a height, two Microsoft footprints: Microsoft's footprints,
            // no heights (1/3).
            ((6, 1), vec![(120, 0, 0, 3, 3), (0, 0, 0, 2, 2), (0, 0, 0, 2, 2)]),
            // OSM's storeys on both (no height); a part not counted.
            ((7, 1), vec![(0, 4, 0, 3, 3), (0, 2, 0, 3, 3), (0, 0, flag::PART, 3, 3)]),
            // Houston: OSM's heights on four, Esri's on four Microsoft footprints, one with none:
            // both datasets' heights, Microsoft's footprints (5 of 9).
            ((8, 1), [vec![(100, 0, 0, 3, 3); 4], vec![(100, 0, 0, 1, 2); 4], vec![(0, 0, 0, 2, 2)]].concat()),
            // A stray OSM building among ten Microsoft ones: Microsoft's alone.
            ((9, 1), [vec![(0, 0, 0, 2, 2); 10], vec![(0, 0, 0, 3, 3)]].concat()),
            // An OSM footprint whose height is Esri's: OSM's footprint, Esri's height.
            ((200, 9), vec![(150, 0, 0, 1, 3)]),
            // Only parts: no buildings.
            ((201, 9), vec![(0, 0, flag::PART, 3, 3)]),
            // A tie of Esri's and Microsoft's footprints: Esri's (first by name), undeclared.
            ((202, 9), vec![(0, 0, 0, 1, 1), (0, 0, 0, 2, 2)]),
            // Three OSM heights, two OSM storeys alone: heights, and the storeys (two of two).
            ((203, 9), [vec![(100, 0, 0, 3, 3); 3], vec![(0, 3, 0, 3, 3); 2]].concat()),
        ],
    );
    let s = specs5();
    let m = masks(&mut o, "buildings", &s, &|_, _, _| {}).unwrap();
    assert_eq!((m.read, m.kept, m.left_out.len()), (1, 0, 0));
    let t = (33, 22);
    assert_eq!(cells(&m.masks[0], t), [(5, 1), (7, 1), (200, 9), (203, 9)]);
    assert_eq!(cells(&m.masks[1], t), [(5, 1), (8, 1), (203, 9)]);
    assert_eq!(cells(&m.masks[2], t), [(7, 1), (203, 9)]);
    assert_eq!(cells(&m.masks[3], t), [(6, 1), (8, 1), (9, 1)]);
    assert_eq!(cells(&m.masks[4], t), [(8, 1), (200, 9)]);
    // Side by side, one run.
    assert_eq!(m.masks[3].tiles[&t].row(1), [(6, 7), (8, 10)]);
    assert_eq!(m.undeclared.get(&("footprints".into(), ESRI.into())), Some(&1));
    assert_eq!(m.undeclared.get(&("heights".into(), MS.into())), None);
    // Again: the record's cells, nothing read, the same masks and the same record.
    let rec = o.get(&codecs::available::masks_logical("buildings")).unwrap().to_string();
    let m2 = masks(&mut o, "buildings", &s, &|_, _, _| {}).unwrap();
    assert_eq!((m2.read, m2.kept), (0, 1));
    for (a, b) in m.masks.iter().zip(&m2.masks) {
        assert_eq!((&a.id, &a.tiles), (&b.id, &b.tiles));
    }
    assert_eq!(o.get(&codecs::available::masks_logical("buildings")).unwrap(), rec);
    // A second file: it alone read; its masks beside the first's.
    put(&mut o, d.path(), (34, 22), &srcs, &[((0, 0), vec![(0, 0, 0, 2, 2)])]);
    let m3 = masks(&mut o, "buildings", &s, &|_, _, _| {}).unwrap();
    assert_eq!((m3.read, m3.kept), (1, 1));
    assert_eq!(cells(&m3.masks[3], (34, 22)), [(0, 0)]);
    assert_eq!(cells(&m3.masks[3], t), [(6, 1), (8, 1), (9, 1)]);
    // A spec changed: every file read again.
    let mut s2 = specs5();
    s2[1].block.holds = "storeys".into();
    let m4 = masks(&mut o, "buildings", &s2, &|_, _, _| {}).unwrap();
    assert_eq!((m4.read, m4.kept), (2, 0));
    assert_eq!(cells(&m4.masks[1], t), [(7, 1), (203, 9)]);
    // A normalized file made again, its side file still the old one's (naming another): left
    // out, said in the record, its tile in no mask; then its own side file: read.
    let (work, fsrc) = put_work(&mut o, d.path(), (34, 22), &srcs, &[((0, 0), vec![(0, 0, 0, 2, 2); 2])]);
    let said = std::sync::Mutex::new(Vec::new());
    let m5 = masks(&mut o, "buildings", &s2, &|d, n, l| said.lock().unwrap().push((d, n, l))).unwrap();
    assert_eq!((m5.read, m5.kept, &m5.left_out[..]), (0, 1, &[(34, 22)][..]));
    assert_eq!(said.into_inner().unwrap(), [(1, 1, 1)]);
    assert!(!m5.masks[3].tiles.contains_key(&(34, 22)));
    let l = codecs::available::masks_logical("buildings");
    let rec = zstd::decode_all(&o.path(o.get(&l).unwrap()).read().unwrap()[..]).unwrap();
    assert_eq!(rec[rec.len() - 12..], [1, 0, 0, 0, 34, 0, 0, 0, 22, 0, 0, 0]);
    put_side(&mut o, d.path(), (34, 22), &work, &fsrc);
    let m5 = masks(&mut o, "buildings", &s2, &|_, _, _| {}).unwrap();
    assert_eq!((m5.read, m5.kept, m5.left_out.len()), (1, 1, 0));
    assert_eq!(cells(&m5.masks[3], (34, 22)), [(0, 0)]);
    // A side file gone: its tile left out, nothing read.
    o.remove(&crate::bld::side_logical(34, 22));
    let m5 = masks(&mut o, "buildings", &s2, &|_, _, _| {}).unwrap();
    assert_eq!((m5.read, m5.kept, &m5.left_out[..]), (0, 1, &[(34, 22)][..]));
    // A file gone: its tile gone from the masks and the record.
    o.remove(&crate::bld::work_logical(34, 22));
    let m6 = masks(&mut o, "buildings", &s2, &|_, _, _| {}).unwrap();
    assert_eq!((m6.read, m6.kept, m6.left_out.len()), (0, 1, 0));
    // No mask piece: nothing read, the record gone.
    let m7 = masks(&mut o, "buildings", &[], &|_, _, _| {}).unwrap();
    assert!(m7.masks.is_empty() && m7.read == 0 && m7.kept == 0);
    assert!(o.get(&codecs::available::masks_logical("buildings")).is_none());
}

#[test]
fn a_record_unreadable_or_of_other_specs_is_read_again() {
    let d = tempfile::tempdir().unwrap();
    let mut o = out(d.path());
    put(&mut o, d.path(), (10, 20), &[OSM], &[((3, 4), vec![(0, 0, 0, 1, 1)])]);
    let s = specs5();
    masks(&mut o, "buildings", &s, &|_, _, _| {}).unwrap();
    let l = codecs::available::masks_logical("buildings");
    let b = o.path(o.get(&l).unwrap()).read().unwrap();
    // Its own bytes decode to what it holds; other specs' digest holds nothing; cut short, refused.
    let digest = specs_digest(&s);
    assert_eq!(decode(&b, &digest, s.len()).unwrap().len(), 1);
    assert!(decode(&b, &specs_digest(&s[..2]), 2).unwrap().is_empty());
    assert!(decode(&b[..b.len() - 3], &digest, s.len()).is_err());
    o.put_bytes(&l, "bin", b"not a record").unwrap();
    let m = masks(&mut o, "buildings", &s, &|_, _, _| {}).unwrap();
    assert_eq!((m.read, m.kept), (1, 0));
    assert_eq!(cells(&m.masks[0], (10, 20)), [(3, 4)]);
}
#[test]
fn specs_come_from_the_overture_blocks_readings() {
    let pieces = br#"{"type": "FeatureCollection", "source": "overture", "theme": "buildings",
        "read": [{"piece": "osm-heights", "method": "overture", "dataset": "OpenStreetMap", "holds": "heights",
        "grain": "z14 tiles"}, {"piece": "city", "method": "file"}],
        "features": [{"type": "Feature", "geometry": null, "properties": {"piece": "osm-heights", "extent": "mask",
        "availability": "open", "detail": "heights"}}, {"type": "Feature", "geometry": null, "properties":
        {"piece": "city", "extent": "unknown", "availability": "open", "detail": "lod2"}}]}"#;
    let s = specs("overture", pieces).unwrap();
    assert_eq!(s, [spec("osm-heights", OSM, "heights")]);
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
    let ours = |l: &String| l.starts_with(crate::bld::WORK) || l.starts_with(crate::bld::SIDE);
    for (l, c) in manifest.iter().filter(|(l, _)| ours(l)) {
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
    let m = masks(&mut o, "buildings", &specs_all, &|_, _, _| {}).unwrap();
    eprintln!(
        "masks: {} files ({} read, {} kept, {} z6 tiles left out) in {:.1?}",
        files,
        m.read,
        m.kept,
        m.left_out.len(),
        t.elapsed()
    );
    for k in &m.masks {
        eprintln!("  {}: {} z6 tiles", k.id, k.tiles.len());
    }
    for ((holds, dataset), n) in &m.undeclared {
        eprintln!("  undeclared: {n} buildings' {holds} of {dataset}");
    }
    let t = std::time::Instant::now();
    let again = masks(&mut o, "buildings", &specs_all, &|_, _, _| {}).unwrap();
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
