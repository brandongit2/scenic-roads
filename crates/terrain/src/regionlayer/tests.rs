use super::*;
use det::Det;
use std::path::Path;
use store::nasfs::NasRoot;

fn e7(lon: f64, lat: f64) -> [i32; 2] {
    [(lon * 1e7).round() as i32, (lat * 1e7).round() as i32]
}

/// Two regions either side of a border at `x` (bent at its middle by `bend` degrees), in the box.
fn pair(a: &str, b: &str, w: f64, s: f64, e: f64, n: f64, x: f64, bend: f64) -> Vec<Region> {
    let m = e7(x + bend, (s + n) / 2.0);
    let west = vec![e7(w, s), e7(x, s), m, e7(x, n), e7(w, n)];
    let east = vec![e7(x, s), e7(e, s), e7(e, n), e7(x, n), m];
    vec![
        Region { id: a.into(), folders: vec![], rings: vec![west] },
        Region { id: b.into(), folders: vec![], rings: vec![east] },
    ]
}

fn out(root: &Path) -> Out {
    Out::unbound(&NasRoot::local_for_test(root), &root.join("scratch")).unwrap()
}

fn run(root: &Path, regions: &[Region]) -> Made {
    let mut o = out(root);
    let m = build(&mut o, regions, &|_, _, _| {}).unwrap();
    o.save().unwrap();
    m
}

/// Whether pack `l` (`layers/regions/<scope>/<z>-<x>-<y>`) covers the point.
fn covers(l: &str, lon: f64, lat: f64) -> bool {
    let t: Vec<u32> = l.rsplit('/').next().unwrap().split('-').map(|v| v.parse().unwrap()).collect();
    let (wx, wy) = roadcore::geo::merc(lon, lat);
    let n = f64::from(1u32 << t[0]);
    (wx * n) as u32 == t[1] && (wy * n) as u32 == t[2]
}

#[test]
fn one_region_changed_writes_only_its_packs() {
    let d = tempfile::tempdir().unwrap();
    let mut regions = pair("a1", "a2", 10.0, 40.0, 11.0, 41.0, 10.5, 0.1);
    regions.extend(pair("b1", "b2", -100.0, 30.0, -99.0, 31.0, -99.5, 0.1));
    regions.sort_by(|a, b| a.id.cmp(&b.id));
    let first = run(d.path(), &regions);
    assert!(first.written.len() > 3 && first.kept == 0, "{first:?}");
    assert!(first.written.iter().any(|l| l.contains("/root/")) && first.written.iter().any(|l| l.contains("/hi/")));
    // Nothing moved: nothing written.
    let again = run(d.path(), &regions);
    assert!(again.written.is_empty() && again.dropped.is_empty() && again.kept == first.written.len(), "{again:?}");
    // The b regions' border bent the other way: the root and the packs over b alone.
    let mut moved = pair("a1", "a2", 10.0, 40.0, 11.0, 41.0, 10.5, 0.1);
    moved.extend(pair("b1", "b2", -100.0, 30.0, -99.0, 31.0, -99.5, -0.1));
    moved.sort_by(|a, b| a.id.cmp(&b.id));
    let m = run(d.path(), &moved);
    assert!(!m.written.is_empty() && m.written.len() < first.written.len(), "{m:?}");
    for l in &m.written {
        assert!(l.contains("/root/") || covers(l, -99.5, 30.5), "{l} written, not over b");
        assert!(!covers(l, 10.5, 40.5) || l.contains("/root/"), "{l} over a written");
    }
    // b gone: its packs go.
    let left: Vec<Region> = moved.into_iter().filter(|r| r.id.starts_with('a')).collect();
    let m = run(d.path(), &left);
    assert!(!m.dropped.is_empty() && m.dropped.iter().all(|l| covers(l, -99.5, 30.5)), "{m:?}");
    let o = out(d.path());
    let packed: Vec<&String> = o.manifest.keys().filter(|l| l.starts_with("layers/regions/")).collect();
    assert!(packed.iter().all(|l| !covers(l, -99.5, 30.5) || l.contains("/root/")));
}

#[test]
fn a_region_read_with_its_inside_on_the_left() {
    use inputs::inputs::regions::stored::{Geometry, Shape};
    let sq = |w: f64, s: f64, e: f64, n: f64| vec![e7(w, s), e7(e, s), e7(e, n), e7(w, n)];
    let cw = |mut r: Vec<[i32; 2]>| {
        r.reverse();
        r
    };
    let shape = |rings: Vec<Vec<[i32; 2]>>, water: bool| Shape {
        from: "x".into(),
        country: String::new(),
        tolerance_m: 0,
        rings,
        water,
    };
    let geometry =
        |shapes| Geometry { id: "r".into(), osm: "2026-09-28".into(), shapes, relations: vec![], ways: vec![] };
    let a2 = |r: &Vec<[i32; 2]>| -> i128 {
        (0..r.len())
            .map(|i| {
                r[i][0] as i128 * r[(i + 1) % r.len()][1] as i128 - r[(i + 1) % r.len()][0] as i128 * r[i][1] as i128
            })
            .sum()
    };
    // Land of two cells' pieces (one drawn clockwise: even-odd reads it all the same), a lake in the
    // first (its ring counter-clockwise: a hole all the same), and water touching them along the
    // coast: read as they are, outer rings counter-clockwise, the hole clockwise.
    let g = geometry(vec![
        shape(vec![sq(0.0, 0.0, 1.0, 1.0), cw(sq(1.0, 0.0, 2.0, 1.0)), sq(0.25, 0.25, 0.75, 0.75)], false),
        shape(vec![sq(0.0, 1.0, 2.0, 2.0)], true),
    ]);
    let rings = area_of(&g);
    assert_eq!(rings.len(), 4, "{rings:?}");
    let holes: Vec<bool> = rings.iter().map(|r| a2(r) < 0).collect();
    assert_eq!(holes, [false, false, true, false]);
    // As the topology reads them: the edges between the pieces go; the outline (8) and the lake (2)
    // stay.
    let t = fmt::topo::build(&[Region { id: "r".into(), folders: vec![], rings }]);
    let len: f64 = t
        .arcs
        .iter()
        .flat_map(|a| a.pts.windows(2))
        .map(|w| f64::from(w[1][0] - w[0][0]).dhypot(f64::from(w[1][1] - w[0][1])) * 1e-7)
        .sum();
    assert!((len - 10.0).abs() < 1e-6, "{len}");
    // Two parts overlapping: unioned, one ring.
    let g = geometry(vec![shape(vec![sq(0.0, 0.0, 2.0, 2.0)], false), shape(vec![cw(sq(1.0, 1.0, 3.0, 3.0))], false)]);
    let rings = area_of(&g);
    assert_eq!(rings.len(), 1, "{rings:?}");
    assert!(a2(&rings[0]) > 0);
    // A point past the antimeridian held to it.
    let g = geometry(vec![shape(
        vec![vec![e7(179.0, 0.0), [fmt::HALF + 1, 0], [fmt::HALF + 1, 10_000_000], e7(179.0, 1.0)]],
        false,
    )]);
    assert!(area_of(&g).iter().flatten().all(|p| p[0] <= fmt::HALF));
}

/// The real regions, all of them, made into the layer (a benchmark run by hand, on the Mac mini
/// where the NAS is: docs/steps.md §15 has its figures). `SCENIC_REGIONS_NAS`: the project folder;
/// `SCENIC_REGIONS_INDEX`: the regions' accepted index (its content name); `SCENIC_REGIONS_OUT`: a
/// scratch root the packs go into. With `SCENIC_REGIONS_MAP` (a map snapshot, `map/<seq>.json.zst`),
/// that map with the layer added is written as the scratch root's own (a `--root` server's).
#[test]
#[ignore]
fn real_regions() {
    let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} not set"));
    let nas = NasRoot::from_arg(std::path::PathBuf::from(env("SCENIC_REGIONS_NAS")));
    let mut m = std::collections::BTreeMap::new();
    m.insert(inputs::inputs::logical(inputs::inputs::regions::UNIT), env("SCENIC_REGIONS_INDEX"));
    let a = inputs::inputs::open(&nas, &m, inputs::inputs::regions::UNIT).unwrap().unwrap();
    let t = std::time::Instant::now();
    let regions = regions(&nas, &a).unwrap();
    eprintln!("read {} regions in {:.1} s", regions.len(), t.elapsed().as_secs_f64());
    let root = std::path::PathBuf::from(env("SCENIC_REGIONS_OUT"));
    let mut o = out(&root);
    let t2 = std::time::Instant::now();
    let made = build(&mut o, &regions, &|d, n, what| eprintln!("{:.1} s: {d}/{n} {what}", t2.elapsed().as_secs_f64()))
        .unwrap();
    o.save().unwrap();
    eprintln!("built in {:.1} s", t2.elapsed().as_secs_f64());
    println!("{}", serde_json::to_string_pretty(&made).unwrap());
    let Ok(map) = std::env::var("SCENIC_REGIONS_MAP") else { return };
    let (m, _) = store::map::Snapshot::decode(&std::fs::read(&map).unwrap()).unwrap();
    let mut cat = m.catalogue().unwrap();
    let mut layer = store::catalogue::Layer {
        encoding: fmt::ENCODING.into(),
        minzoom: 0,
        maxzoom: fmt::MAXZ,
        ..Default::default()
    };
    for (l, c) in o.manifest.iter().filter(|(l, _)| l.starts_with("layers/regions/")) {
        let size = std::fs::metadata(root.join(c)).unwrap().len();
        cat.files.insert(l.clone(), store::catalogue::FileRef { file: c.clone(), size, fmt: 1, ..Default::default() });
        let key = l.rsplit('/').next().unwrap().replace('-', "/");
        match l.split('/').nth(2).unwrap() {
            "root" => layer.root = Some(l.clone()),
            "lo" => drop(layer.lo.insert(key, l.clone())),
            _ => drop(layer.hi.insert(key, l.clone())),
        }
    }
    cat.layers.insert(fmt::LAYER.into(), layer);
    cat.n += 1;
    store::map::write_root_map(&root, &store::map::Map::of(&cat).unwrap()).unwrap();
    eprintln!("map {} written with the layer", cat.n);
}
