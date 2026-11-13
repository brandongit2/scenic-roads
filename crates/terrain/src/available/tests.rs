use super::*;
use codecs::available::Grid;
use std::path::Path;
use store::nasfs::NasRoot;

type Rings = Vec<Vec<[f64; 2]>>;

/// A box's ring, counter-clockwise.
fn square(w: f64, s: f64, e: f64, n: f64) -> Vec<[f64; 2]> {
    vec![[w, s], [e, s], [e, n], [w, n], [w, s]]
}

/// Source `source`'s pieces (id, polygons) as a `.geojson`, read as the step reads one.
fn source(source: &str, ps: &[(&str, Vec<Rings>)]) -> Vec<Piece> {
    let feats: Vec<Value> = ps
        .iter()
        .map(|(id, polys)| {
            serde_json::json!({"type": "Feature", "properties": {"piece": id, "extent": "index", "availability": "open"},
                "geometry": {"type": "MultiPolygon", "coordinates": polys}})
        })
        .collect();
    let fc = serde_json::json!({"type": "FeatureCollection", "source": source, "theme": "terrain", "read": [], "features": feats});
    pieces(source, &serde_json::to_vec(&fc).unwrap()).unwrap()
}

fn out(root: &Path) -> Out {
    Out::unbound(&NasRoot::local_for_test(root), &root.join("scratch")).unwrap()
}

fn run(root: &Path, pieces: &[Piece]) -> Made {
    let mut o = out(root);
    let m = build(&mut o, "terrain", pieces, &|_, _| {}).unwrap();
    o.save().unwrap();
    m
}

/// The stored tile z/x/y of a layer, from the records at `root`.
fn stored_tile(root: &Path, layer: &str, z: u8, x: u32, y: u32) -> Option<Tile> {
    let o = out(root);
    let (_, grid) = fmt::of_layer(layer).unwrap();
    let (scope, key) = fmt::pack_of(grid, z, x, y)?;
    let l = format!("layers/{layer}/{scope}/{}", key.replace('/', "-"));
    let c = o.get(&l)?;
    let b = o.path(c).read().unwrap();
    let idx = store::pack::PackIndex::parse_bytes(&b).unwrap();
    let e = idx.find(z, x, y)?;
    Some(Tile::decode(&b[e.offset as usize..][..e.len as usize]).unwrap())
}

/// The pieces the layers say are at a point.
fn at(root: &Path, lon: f64, lat: f64) -> Vec<String> {
    let (x, y) = roadcore::geo::merc(lon, lat);
    let w = f64::from(MERC_W);
    let mut get = |z, x, y| Ok(stored_tile(root, "available-terrain", z, x, y));
    fmt::pieces_at(Grid::Mercator, (x * w) as u64, (y * w) as u64, &mut get).unwrap()
}

fn at_pole(root: &Path, pole: Pole, lon: f64, lat: f64) -> Vec<String> {
    let p = polar_px(pole, lon, lat);
    let layer = fmt::polar_layer("terrain", pole.letter());
    let mut get = |z, x, y| Ok(stored_tile(root, &layer, z, x, y));
    fmt::pieces_at(Grid::Polar(pole.letter()), p[0] as u64, p[1] as u64, &mut get).unwrap()
}

fn packs(root: &Path) -> BTreeMap<String, String> {
    out(root).manifest.into_iter().filter(|(l, _)| l.starts_with("layers/available-")).collect()
}

#[test]
fn overlapping_pieces_make_sets_and_their_palettes() {
    let d = tempfile::tempdir().unwrap();
    let a = source("a", &[("one", vec![vec![square(2.0, 44.0, 2.4, 44.3)]])]);
    let b = source("b", &[("two", vec![vec![square(2.2, 44.1, 2.6, 44.4)]])]);
    let all = [a, b].concat();
    let m = run(d.path(), &all);
    assert!(m.root && m.packs >= 1, "{m:?}");
    assert_eq!(at(d.path(), 2.1, 44.05), ["a/one"]);
    assert_eq!(at(d.path(), 2.3, 44.2), ["a/one", "b/two"]);
    assert_eq!(at(d.path(), 2.5, 44.35), ["b/two"]);
    assert!(at(d.path(), 2.7, 44.2).is_empty());
    // A z12 tile across both outlines: its palette holds the sets met there, each once, and the
    // ids it names; a pixel indexes it.
    let (x, y) = roadcore::geo::merc(2.4, 44.1);
    let (tx, ty) = ((x * 4096.0) as u32, (y * 4096.0) as u32);
    let t = stored_tile(d.path(), "available-terrain", 12, tx, ty).expect("the z12 tile at a corner is stored");
    let mut sets: Vec<Vec<&str>> =
        t.sets.iter().map(|s| s.iter().map(|&k| t.ids[k as usize].as_str()).collect()).collect();
    sets.sort();
    assert_eq!(sets, [vec![], vec!["a/one"], vec!["a/one", "b/two"], vec!["b/two"]]);
    // A tile all inside both isn't stored: its ancestor's pixel says so.
    let (x, y) = roadcore::geo::merc(2.3, 44.2);
    assert!(stored_tile(d.path(), "available-terrain", 12, (x * 4096.0) as u32, (y * 4096.0) as u32).is_none());
    // Zoomed out, a pixel is the union of what's under it.
    let t0 = stored_tile(d.path(), "available-terrain", 0, 0, 0).unwrap();
    let (x, y) = roadcore::geo::merc(2.3, 44.2);
    assert_eq!(t0.at((x * 256.0) as usize, (y * 256.0) as usize).collect::<Vec<_>>(), ["a/one", "b/two"]);
}

#[test]
fn a_piece_smaller_than_a_pixel_stays_in_the_overviews() {
    let d = tempfile::tempdir().unwrap();
    // (About 100 m a side: a z12 pixel's two or three, a z0 pixel's thousandth.)
    let p = source("tiny", &[("p", vec![vec![square(10.0, 50.0, 10.0014, 50.0009)]])]);
    run(d.path(), &p);
    let (x, y) = roadcore::geo::merc(10.0007, 50.00045);
    for z in 0..=fmt::MAXZ {
        let n = f64::from(1u32 << z);
        let (tx, ty) = ((x * n) as u32, (y * n) as u32);
        let t = fmt::tile_at(Grid::Mercator, z, tx, ty, &mut |z, x, y| {
            Ok(stored_tile(d.path(), "available-terrain", z, x, y))
        })
        .unwrap()
        .unwrap_or_else(|| panic!("z{z}: no tile"));
        let (i, j) = (((x * n).fract() * 256.0) as usize, ((y * n).fract() * 256.0) as usize);
        assert_eq!(t.at(i, j).collect::<Vec<_>>(), ["tiny/p"], "z{z}");
    }
}

#[test]
fn overlapping_polygons_of_one_piece_add_up() {
    let d = tempfile::tempdir().unwrap();
    let p = source("s", &[("p", vec![vec![square(5.0, 5.0, 5.2, 5.2)], vec![square(5.1, 5.1, 5.3, 5.3)]])]);
    run(d.path(), &p);
    assert_eq!(at(d.path(), 5.15, 5.15), ["s/p"], "the overlap is the piece's, not a hole");
    assert_eq!(at(d.path(), 5.05, 5.05), ["s/p"]);
    assert_eq!(at(d.path(), 5.25, 5.25), ["s/p"]);
    assert!(at(d.path(), 5.25, 5.05).is_empty());
    // And a hole is a hole.
    let d = tempfile::tempdir().unwrap();
    let mut ring = square(5.0, 5.0, 5.3, 5.3);
    ring.reverse();
    let p = source("s", &[("p", vec![vec![square(4.9, 4.9, 5.4, 5.4), ring]])]);
    run(d.path(), &p);
    assert!(at(d.path(), 5.15, 5.15).is_empty());
    assert_eq!(at(d.path(), 4.95, 5.15), ["s/p"]);
}

#[test]
fn a_piece_cut_at_the_antimeridian_meets_both_edges() {
    let d = tempfile::tempdir().unwrap();
    let p = source(
        "fiji",
        &[("p", vec![vec![square(179.5, -18.0, 180.0, -17.0)], vec![square(-180.0, -18.0, -179.5, -17.0)]])],
    );
    run(d.path(), &p);
    assert_eq!(at(d.path(), 179.9999, -17.5), ["fiji/p"]);
    assert_eq!(at(d.path(), -179.9999, -17.5), ["fiji/p"]);
    assert!(at(d.path(), 0.0, -17.5).is_empty());
    assert!(at(d.path(), 179.4, -17.5).is_empty());
    // The z12 columns at both edges hold it.
    let (_, y) = roadcore::geo::merc(0.0, -17.5);
    let ty = (y * 4096.0) as u32;
    assert!(packs(d.path()).contains_key(&fmt::pack_logical("available-terrain", 63, ty >> 6)));
    assert!(packs(d.path()).contains_key(&fmt::pack_logical("available-terrain", 0, ty >> 6)));
}

#[test]
fn the_polar_caps_hold_the_pieces_past_the_tiles() {
    let d = tempfile::tempdir().unwrap();
    // The north: a cap from 86° to the pole, closed along 90° (as the producer closes one).
    let north = vec![vec![[-180.0, 86.0], [180.0, 86.0], [180.0, 90.0], [-180.0, 90.0], [-180.0, 86.0]]];
    // The south: a ring from 80° S to 88° S, its hole round the pole kept (REMA's).
    let south = vec![vec![[-180.0, -88.0], [180.0, -88.0], [180.0, -80.0], [-180.0, -80.0], [-180.0, -88.0]]];
    let p = source("polar", &[("n", vec![north]), ("s", vec![south])]);
    let m = run(d.path(), &p);
    assert_eq!(m.poles, ['n', 's']);
    assert_eq!(at_pole(d.path(), Pole::North, 0.0, 89.9), ["polar/n"]);
    assert_eq!(at_pole(d.path(), Pole::North, 120.0, 87.0), ["polar/n"]);
    assert_eq!(at_pole(d.path(), Pole::North, -60.0, 85.5), Vec::<String>::new());
    assert_eq!(at_pole(d.path(), Pole::South, 0.0, -89.9), Vec::<String>::new(), "the hole round the pole");
    assert_eq!(at_pole(d.path(), Pole::South, 77.0, -86.0), ["polar/s"]);
    assert_eq!(at_pole(d.path(), Pole::South, 77.0, -84.0), ["polar/s"]);
    // In the Mercator tiles: the south's part above 85.05° S, the north's none.
    assert_eq!(at(d.path(), 30.0, -82.0), ["polar/s"]);
    assert!(at(d.path(), 30.0, 85.0).is_empty());
    // Made again with nothing changed: nothing made.
    let again = run(d.path(), &p);
    assert!(again.remade.is_empty() && again.poles.is_empty() && !again.root, "{again:?}");
}

#[test]
fn a_tile_all_of_one_set_is_not_stored() {
    let d = tempfile::tempdir().unwrap();
    // (A box well over one z6 tile, 5.6° a side: its middle z6 tiles are all inside.)
    let p = source("big", &[("p", vec![vec![square(0.5, 30.5, 17.0, 45.5)]])]);
    run(d.path(), &p);
    let (x, y) = roadcore::geo::merc(8.0, 38.0);
    let (tx, ty) = ((x * 64.0) as u32, (y * 64.0) as u32);
    let ps = packs(d.path());
    assert!(!ps.contains_key(&fmt::pack_logical("available-terrain", tx, ty)), "{ps:?}");
    assert_eq!(at(d.path(), 8.0, 38.0), ["big/p"]);
    // Its edges' z6 tiles are.
    let (x, _) = roadcore::geo::merc(0.5, 38.0);
    assert!(ps.contains_key(&fmt::pack_logical("available-terrain", (x * 64.0) as u32, ty)));
}

#[test]
fn a_changed_piece_makes_only_its_tiles_again() {
    let d = tempfile::tempdir().unwrap();
    let a = source("a", &[("p", vec![vec![square(2.0, 44.0, 3.0, 45.0)]])]);
    let b = |e: f64| source("b", &[("p", vec![vec![square(-70.0, 40.0, e, 41.0)]])]);
    let first = run(d.path(), &[a.clone(), b(-69.0)].concat());
    assert!(!first.remade.is_empty());
    let before = packs(d.path());
    // B moved: only B's z6 tiles (as it was, and as it is) are made again; A's packs are as they were.
    let m = run(d.path(), &[a.clone(), b(-68.0)].concat());
    let b_tiles: BTreeSet<String> = tile_keys(&b(-69.0)).into_keys().chain(tile_keys(&b(-68.0)).into_keys()).collect();
    assert!(!m.remade.is_empty());
    for t in &m.remade {
        assert!(b_tiles.contains(t), "{t} isn't B's");
    }
    assert!(m.root, "the root is made again from the mids");
    let after = packs(d.path());
    for t in tile_keys(&a).into_keys() {
        let (x, y) = xy(&t);
        let l = fmt::pack_logical("available-terrain", x, y);
        assert_eq!(before.get(&l), after.get(&l), "{l}");
    }
    assert_eq!(at(d.path(), -68.5, 40.5), ["b/p"]);
    // B gone: its tiles dropped, A's kept.
    let m = run(d.path(), &a);
    assert!(m.remade.is_empty() && !m.dropped.is_empty(), "{m:?}");
    assert!(at(d.path(), -68.5, 40.5).is_empty());
    assert_eq!(at(d.path(), 2.5, 44.5), ["a/p"]);
    // And the same pieces made from nothing make the same files.
    let fresh = tempfile::tempdir().unwrap();
    run(fresh.path(), &a);
    assert_eq!(packs(fresh.path()), packs(d.path()));
}

/// The real pieces (producers/extents/terrain/, as the unit takes them), each kept to the polygons
/// `keep` keeps (a piece left none goes).
fn real_pieces(keep: impl Fn(&Polygon) -> bool) -> Vec<Piece> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../producers/extents/terrain");
    let mut all = Vec::new();
    let mut names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).collect();
    names.sort();
    for p in names.iter().filter(|p| p.extension().is_some_and(|e| e == "geojson")) {
        let stem = p.file_stem().unwrap().to_string_lossy().to_string();
        all.extend(pieces(&stem, &std::fs::read(p).unwrap()).unwrap());
    }
    for p in &mut all {
        p.polygons.retain(&keep);
    }
    all.retain(|p| !p.polygons.is_empty());
    all
}

/// Builds `all` and checks what any pieces' tiles must hold: every piece in some tile (or a pole's),
/// z0 naming every piece a finer tile does, and each stored tile's pixel holding every piece its
/// stored children's pixels under it do (sampled every 7th pixel). One pack is in memory at once
/// beside the root (a pack's tiles' parents are in it, its z6 tile's in the root).
fn real_checks(all: &[Piece]) -> Made {
    let d = tempfile::tempdir().unwrap();
    let t = std::time::Instant::now();
    let m = run(d.path(), all);
    eprintln!(
        "{} pieces: {} z6 tiles, {} packs, {} tiles in {:.1?}",
        all.len(),
        m.remade.len(),
        m.packs,
        m.tiles,
        t.elapsed()
    );
    let o = out(d.path());
    let read = |l: &str| -> BTreeMap<(u8, u32, u32), Tile> {
        let b = o.path(o.get(l).unwrap()).read().unwrap();
        let idx = store::pack::PackIndex::parse_bytes(&b).unwrap();
        assert!(!idx.entries.is_empty(), "{l}");
        idx.entries
            .iter()
            .map(|e| (e.zxy(), Tile::decode(&b[e.offset as usize..][..e.len as usize]).unwrap()))
            .collect()
    };
    // Child c (at cz/cx/cy) against its parent's pixels over it.
    let check = |parent: &Tile, (cz, cx, cy): (u8, u32, u32), c: &Tile| {
        let (dx, dy) = ((cx % 2) as usize, (cy % 2) as usize);
        for j in (0..SIDE).step_by(7) {
            for i in (0..SIDE).step_by(7) {
                let p: BTreeSet<&str> = parent.at((dx * SIDE + i) / 2, (dy * SIDE + j) / 2).collect();
                for id in c.at(i, j) {
                    assert!(p.contains(id), "{cz}/{cx}/{cy}: {id} not over it");
                }
            }
        }
    };
    let mut ids_of: BTreeMap<u8, BTreeSet<String>> = BTreeMap::new();
    let mut checked = 0;
    let root = read(&fmt::root_logical(&fmt::layer("terrain")));
    let packs: Vec<String> = o
        .manifest
        .iter()
        .map(|(l, _)| l.clone())
        .filter(|l| l.starts_with(&format!("layers/{}/hi/", fmt::layer("terrain"))))
        .collect();
    let mut each = |tiles: &BTreeMap<(u8, u32, u32), Tile>| {
        for (&k, t) in tiles {
            ids_of.entry(k.0).or_default().extend(t.ids.iter().cloned());
            if k.0 == 0 {
                continue;
            }
            let up = (k.0 - 1, k.1 / 2, k.2 / 2);
            if let Some(p) = tiles.get(&up).or_else(|| root.get(&up)) {
                check(p, k, t);
                checked += 1;
            }
        }
    };
    each(&root);
    for l in &packs {
        each(&read(l));
    }
    let drawn: BTreeSet<String> = all.iter().map(|p| p.id.clone()).collect();
    let anywhere: BTreeSet<String> = ids_of.values().flatten().cloned().collect();
    let polar: BTreeSet<String> = ['n', 's']
        .iter()
        .filter_map(|&p| stored_tile(d.path(), &fmt::polar_layer("terrain", p), 0, 0, 0))
        .flat_map(|t| t.ids)
        .collect();
    let missing: Vec<&String> = drawn.iter().filter(|p| !anywhere.contains(*p) && !polar.contains(*p)).collect();
    assert!(missing.is_empty(), "pieces in no tile: {missing:?}");
    // The union overview covers what the tiles do (a piece whose every tile is uniform is in an
    // ancestor's pixels, and so in z0).
    for id in ids_of.iter().filter(|(z, _)| **z > 0).flat_map(|(_, s)| s) {
        assert!(ids_of[&0].contains(id), "{id} is in a finer tile, not in z0");
    }
    assert!(checked > 100, "{checked} tiles checked against their parents");
    m
}

/// The real pieces' polygons lying round Vancouver (124-122.5 W, 48.5-50 N): two sources' outlines
/// overlapping, at a size the test budget holds. The same checks on every piece are
/// `every_real_piece`, a benchmark run by hand.
#[test]
fn the_real_pieces_round_vancouver() {
    let inside = |p: &Polygon| p.bbox[0] >= -124.0 && p.bbox[1] >= 48.5 && p.bbox[2] <= -122.5 && p.bbox[3] <= 50.0;
    let all = real_pieces(inside);
    let sources: BTreeSet<&str> = all.iter().filter_map(|p| p.id.split('/').next()).collect();
    assert!(all.len() > 3 && sources.len() > 1, "{} pieces of {sources:?}", all.len());
    real_checks(&all);
}

/// Every real piece: the step at live scale, a benchmark (`-- --ignored`).
#[test]
#[ignore]
fn every_real_piece() {
    let all = real_pieces(|_| true);
    assert!(all.len() > 900, "{}", all.len());
    let m = real_checks(&all);
    assert!(m.poles.contains(&'n') && m.poles.contains(&'s'));
}
