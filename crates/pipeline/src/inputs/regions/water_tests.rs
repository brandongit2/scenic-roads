//! Land and water (docs/inputs.md §6.1.3, §6.1.4): the new parts and operations one by one, and a
//! small folder resolved whole: a nearest-coast split, a disputed zone of its own, a 12 nmi ring
//! cut at the median line, a lake, a leftover of land, a land slice and the high seas as the rest,
//! the Earth covered once.

use super::data::Datasets;
use super::file::{parse, Part};
use super::geom::{self, Area};
use super::tests::outlines;
use super::*;
use std::collections::HashMap;

const PASS: &str = "2026-09-28";

fn e7(v: f64) -> i64 {
    (v * 1e7).round() as i64
}

fn bx(w: f64, s: f64, e: f64, n: f64) -> Vec<geom::Ring> {
    vec![vec![[e7(w), e7(s)], [e7(e), e7(s)], [e7(e), e7(n)], [e7(w), e7(n)]]]
}

fn deg_sq(w: f64, s: f64, e: f64, n: f64) -> Vec<Vec<[f64; 2]>> {
    vec![vec![[w, s], [e, s], [e, n], [w, n], [w, s]]]
}

fn area_of(g: &stored::Geometry) -> Area {
    geom::union_all(
        &g.shapes
            .iter()
            .map(|s| {
                Area::from_rings(
                    &s.rings.iter().map(|r| r.iter().map(|p| [p[0] as i64, p[1] as i64]).collect()).collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>(),
    )
}

#[test]
fn the_new_parts_parse_and_print() {
    let r = parse(
        "w.toml",
        r#"id = "w"
name = "W"
outline = ["osm:1", "osm-way:177945817"]
water = [{ nearest = "eez:5695", to = ["land:w"], than = ["land:crimea"], step = "2 km" },
         { minus = ["eez:8493", "group:north-america/canada"] },
         { intersect = ["ne-lake:1159106863", "box:9.0,47.4,9.8,47.8"] },
         { union = ["ts12:49001", { buffer = "osm:3", by = "200 nmi" }] }]
"#,
    )
    .unwrap();
    assert!(r.has_water() && !r.is_plain() && r.needs_data());
    assert_eq!(r.ways(), vec![177945817]);
    assert_eq!(r.groups(), vec!["north-america/canada"]);
    assert_eq!(r.lands(), vec!["crimea", "w"]);
    let t = r.outline_text();
    assert_eq!(t[0], "osm:1");
    assert_eq!(t[2], "water {nearest eez:5695 to [land:w] than [land:crimea] step 2 km}");
    assert_eq!(t[4], "water {intersect [ne-lake:1159106863, box:9.0,47.4,9.8,47.8]}");
    let hs = parse("h.toml", "id = \"h\"\nname = \"High seas\"\nwater = [\"rest\"]\n").unwrap();
    assert!(hs.is_rest() && hs.outline.is_empty());
    assert_eq!(
        parse("e.toml", "id = \"e\"\nname = \"E\"\noutline = [\"earth\"]\n").unwrap().outline,
        vec![Part::Earth]
    );
    let bad = |body: &str| {
        parse("x.toml", &format!("id = \"x\"\nname = \"X\"\n{body}"))
            .unwrap_err()
            .iter()
            .map(|f| f.check)
            .collect::<Vec<_>>()
    };
    assert_eq!(bad("outline = [\"rest\"]\n"), vec!["part"]);
    assert_eq!(bad("outline = [\"box:10,0,5,1\"]\n"), vec!["part"]);
    assert_eq!(bad("outline = [\"eez:abc\"]\n"), vec!["part"]);
    assert_eq!(bad("outline = [{ union = [] }]\n"), vec!["part"]);
    assert_eq!(bad("outline = [{ nearest = \"eez:1\", to = [\"osm:1\"] }]\n"), vec!["part"]);
    assert_eq!(
        bad("outline = [{ nearest = \"eez:1\", to = [\"osm:1\"], than = [\"osm:2\"], step = \"10 m\" }]\n"),
        vec!["part"]
    );
    assert_eq!(bad("outline = [{ buffer = \"osm:1\", by = \"201 nmi\" }]\n"), vec!["part"]);
    assert_eq!(bad("outline = [\"land:x\"]\n"), vec!["ref"]);
    assert_eq!(bad(""), vec!["id"]);
    assert!(water::parse_rules("cover = \"earth\"\n").is_ok());
    assert!(water::parse_rules("cover = \"mars\"\n").is_err());
    assert!(water::parse_rules("covers = \"earth\"\n").is_err());
}

#[test]
fn nearest_splits_at_the_median_line() {
    let within = Area::from_rings(&bx(-1.0, -1.0, 4.0, 2.0));
    let a = Area::from_rings(&bx(0.0, 0.0, 1.0, 1.0));
    let b = Area::from_rings(&bx(2.0, 0.0, 3.0, 1.0));
    let n = geom::nearest(&within, &a, &b, 1000.0);
    assert!(n.contains([e7(1.45), e7(0.5)]) && n.contains([e7(-0.5), e7(1.5)]));
    assert!(!n.contains([e7(1.55), e7(0.5)]) && !n.contains([e7(3.5), e7(0.5)]));
    // Within a step of the median (x = 1.5).
    assert!(n.contains([e7(1.49), e7(0.5)]) && !n.contains([e7(1.51), e7(0.5)]));
    // Its complement and it make the whole, with no overlap.
    let m = geom::nearest(&within, &b, &a, 1000.0);
    assert!(n.intersect(&m).is_empty() || geom::area_km2(&n.intersect(&m)) < 1.0);
    assert!((geom::area_km2(&n.union(&m)) - geom::area_km2(&within)).abs() < 1.0);
    // Nothing to be nearer than: all; nothing to be near: none.
    assert_eq!(geom::nearest(&within, &a, &Area::default(), 1000.0), within);
    assert!(geom::nearest(&within, &Area::default(), &b, 1000.0).is_empty());
}

#[test]
fn boxes_areas_and_wide_buffers() {
    // A degree square at the equator: 111.2 km a side on the authalic sphere.
    let sq = geom::box_area([0, 0, 10_000_000, 10_000_000]);
    assert!((geom::area_km2(&sq) - 12_364.0).abs() < 15.0, "{}", geom::area_km2(&sq));
    assert!(
        (geom::area_km2(&geom::earth()) - 510_065_622.0).abs() / 510_065_622.0 < 1e-4,
        "{}",
        geom::area_km2(&geom::earth())
    );
    // 200 nmi round a small island: the point 199 nmi east in, 201 out.
    let isl = Area::from_rings(&bx(0.0, 0.0, 0.01, 0.01));
    let b = geom::buffer_area(&isl, 370_400.0);
    let at = |nmi: f64| geom::destination([e7(0.01), e7(0.005)], std::f64::consts::FRAC_PI_2, nmi * 1852.0);
    assert!(b.contains(at(199.0)) && !b.contains(at(201.0)));
    // The shared edge of two touching squares: a side, 111 km.
    let a = Area::from_rings(&bx(0.0, 0.0, 1.0, 1.0));
    let c = Area::from_rings(&bx(1.0, 0.0, 2.0, 1.0));
    assert!((geom::shared_edge_km(&a, &c, 100.0) - 110.6).abs() < 2.0, "{}", geom::shared_edge_km(&a, &c, 100.0));
}

/// The small world: countries A (relation 1, its OSM outline reaching 0.3° out to sea) and B
/// (2), an island I (3) between them nearer A, a far square D (4) that no region names. The land:
/// A's coast (with a strip north of it no outline has), B's, I and D. Zones: 1 round A, B and I; 2
/// a disputed square of its own. A big lake inside A.
struct World {
    _d: tempfile::TempDir,
    o: crate::outlines::Outlines,
    data: Datasets,
}

fn world() -> World {
    let d = tempfile::tempdir().unwrap();
    let p = outlines(
        d.path(),
        &[
            (1, "AA", deg_sq(0.0, 0.0, 1.5, 1.0)),
            (2, "BB", deg_sq(2.0, 0.0, 3.0, 1.0)),
            (3, "AA", deg_sq(1.6, 1.4, 1.62, 1.42)),
            (4, "DD", deg_sq(20.0, 20.0, 21.0, 21.0)),
        ],
    );
    let land =
        vec![bx(0.0, 0.0, 1.2, 1.05), bx(2.0, 0.0, 3.0, 1.0), bx(1.6, 1.4, 1.62, 1.42), bx(20.0, 20.0, 21.0, 21.0)];
    let zone1 = vec![bx(-1.0, -1.0, 4.0, 2.5)[0].clone()];
    let eez = HashMap::from([(1, zone1), (2, bx(5.0, 5.0, 6.0, 6.0))]);
    let lakes = HashMap::from([(77, bx(0.2, 0.2, 0.4, 0.4)), (78, bx(0.6, 0.6, 0.61, 0.61))]);
    let data = Datasets::from_parts(eez, HashMap::new(), lakes, land, "fixture");
    World { o: crate::outlines::Outlines::open(&p).unwrap(), data, _d: d }
}

fn run(w: &World, files: &[(&str, &str)]) -> resolve::Outcome {
    let m: BTreeMap<String, Vec<u8>> = files.iter().map(|(p, t)| (p.to_string(), t.as_bytes().to_vec())).collect();
    let folder = resolve::Folder::read(m.keys().cloned(), &|p| Ok(m[p].clone())).unwrap();
    let others = BTreeMap::new();
    let ctx = resolve::Context {
        pass: Some(PASS),
        outlines: Some(&w.o),
        extract: None,
        others: &others,
        memo: None,
        names: String::new(),
        data: Some(&w.data),
    };
    resolve::resolve(&folder, &ctx)
}

const A: &str = r#"id = "a"
name = "A"
outline = ["osm:1"]
water = [{ minus = [{ nearest = "eez:1", to = ["land:a"], than = ["land:b", "land:slice"] }, "region:isl"] }]
"#;
const B: &str = r#"id = "b"
name = "B"
outline = ["osm:2"]
minus = ["region:slice"]
water = [{ minus = [{ nearest = "eez:1", to = ["land:b", "land:slice"], than = ["land:a"] }, "region:isl"] }]
"#;
const ISL: &str = r#"id = "isl"
name = "Island"
outline = ["osm:3"]
water = [{ nearest = { intersect = [{ buffer = "osm:3", by = "12 nmi" }, "eez:1"] }, to = ["land:isl"], than = ["land:a", "land:b"] }]
"#;
const SLICE: &str = r#"id = "slice"
name = "Slice"
outline = [{ intersect = ["osm:2", "box:2.0,0.0,3.0,0.5"] }]
"#;
const DISPUTED: &str = "id = \"disputed\"\nname = \"Disputed zone\"\nwater = [\"eez:2\"]\n";
const FAR: &str =
    "id = \"far\"\nname = \"Far\"\noutline = [\"osm:4\"]\nwater = [{ buffer = \"osm:4\", by = \"12 nmi\" }]\n";
const HIGH: &str = "id = \"high-seas\"\nname = \"High seas\"\nwater = [\"rest\"]\n";
const PINS: (&str, &str) = ("pins.toml", "osm = \"2026-09-28\"\n");
const RULES: (&str, &str) = ("rules.toml", "cover = \"earth\"\n");

fn all() -> Vec<(&'static str, &'static str)> {
    vec![
        PINS,
        RULES,
        ("w/a.toml", A),
        ("w/b.toml", B),
        ("w/isl.toml", ISL),
        ("w/slice.toml", SLICE),
        ("disputed.toml", DISPUTED),
        ("far.toml", FAR),
        ("high-seas.toml", HIGH),
    ]
}

#[test]
fn a_small_world_resolves_and_covers_the_earth_once() {
    let w = world();
    let out = run(&w, &all());
    assert!(out.problems.is_empty(), "{:#?}", out.problems);
    let at = |id: &str, x: f64, y: f64| area_of(&out.resolved[id].geometry).contains([e7(x), e7(y)]);
    let owner =
        |x: f64, y: f64| -> Vec<&str> { out.resolved.keys().filter(|id| at(id, x, y)).map(String::as_str).collect() };
    // The nearest-coast split: A's coast at 1.2°, B's at 2.0°: the median at 1.6°.
    assert_eq!(owner(1.5, 0.5), vec!["a"], "A's OSM outline at sea is water by the split");
    assert_eq!(owner(1.7, 0.5), vec!["b"]);
    assert_eq!(owner(-0.5, -0.5), vec!["a"]);
    // The land: A's, B's less the slice, the slice; the leftover strip north of A joins A.
    assert_eq!(owner(0.9, 0.9), vec!["a"]);
    assert_eq!(owner(0.9, 1.02), vec!["a"], "the strip no outline has");
    assert_eq!(owner(2.5, 0.25), vec!["slice"]);
    assert_eq!(owner(2.5, 0.75), vec!["b"]);
    // The lake over 100 km² in A's outline is A's water (and A's either way); the small one land.
    assert_eq!(owner(0.3, 0.3), vec!["a"]);
    // The island's 12 nmi, cut at the median with A's nearer coast.
    assert_eq!(owner(1.61, 1.41), vec!["isl"]);
    assert_eq!(owner(1.61, 1.55), vec!["isl"]);
    // The disputed zone; the far land and its 12 nmi; the rest is the high seas.
    assert_eq!(owner(5.5, 5.5), vec!["disputed"]);
    assert_eq!(owner(20.5, 20.5), vec!["far"]);
    assert_eq!(owner(21.1, 20.5), vec!["far"]);
    assert_eq!(owner(50.0, 50.0), vec!["high-seas"]);
    assert_eq!(owner(4.5, 3.0), vec!["high-seas"]);
    // Once: the areas sum to the Earth's.
    let sum: f64 = out.resolved.values().map(|r| geom::area_km2(&area_of(&r.geometry))).sum();
    assert!((sum - geom::area_km2(&geom::earth())).abs() < 5.0, "{sum}");
    // Twice the same bytes.
    let again = run(&w, &all());
    for (id, r) in &out.resolved {
        assert_eq!(r.geometry.encode(), again.resolved[id].geometry.encode(), "{id}");
    }
}

#[test]
fn overlapping_water_or_an_uncovered_sea_is_an_error() {
    let w = world();
    // B's water not less the island's: the island's ring at sea in two regions.
    let b2 = B.replace(", \"region:isl\"", "");
    let mut files = all();
    files[3] = ("w/b.toml", &b2);
    let out = run(&w, &files);
    assert!(out.problems.iter().any(|p| p.check == "rg-cover" && p.found == vec!["overlap"]), "{:?}", out.problems);
    // No rest: the open sea is in no region.
    let files: Vec<_> = all().into_iter().filter(|f| f.0 != "high-seas.toml").collect();
    let out = run(&w, &files);
    assert!(out.problems.iter().any(|p| p.check == "rg-cover" && p.found == vec!["gap"]), "{:?}", out.problems);
    // Without rules.toml nothing is checked: old and new files side by side.
    let files: Vec<_> = all().into_iter().filter(|f| f.0 != "high-seas.toml" && f.0 != "rules.toml").collect();
    let out = run(&w, &files);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
}

#[test]
fn old_files_resolve_as_before_beside_new_ones() {
    let w = world();
    let old = "id = \"old\"\nname = \"Old\"\noutline = [\"osm:4\"]\n";
    let alone = run(&w, &[PINS, ("old.toml", old)]);
    let mut files = all();
    files.retain(|f| f.0 != "far.toml" && f.0 != "rules.toml");
    files.push(("old.toml", old));
    let mixed = run(&w, &files);
    assert!(mixed.problems.is_empty(), "{:?}", mixed.problems);
    assert_eq!(alone.resolved["old"], mixed.resolved["old"]);
    assert_eq!(alone.resolved["old"].geometry.shapes[0].tolerance_m, 1000);
    // A region with water and no datasets waits, saying why.
    let others = BTreeMap::new();
    let m: BTreeMap<String, Vec<u8>> =
        [PINS, ("w/a.toml", A)].iter().map(|(p, t)| (p.to_string(), t.as_bytes().to_vec())).collect();
    let folder = resolve::Folder::read(m.keys().cloned(), &|p| Ok(m[p].clone())).unwrap();
    let ctx = resolve::Context {
        pass: Some(PASS),
        outlines: Some(&w.o),
        extract: None,
        others: &others,
        memo: None,
        names: String::new(),
        data: None,
    };
    let out = resolve::resolve(&folder, &ctx);
    assert!(out.problems.iter().any(|p| p.check == "rg-data"), "{:?}", out.problems);
}

/// A split place's own waters: the zone less its regions, a stretch only one of them borders handed
/// to it (`hand_out`), the rest the place's waters region.
#[test]
fn a_stretch_one_region_alone_borders_is_its_own() {
    let d = tempfile::tempdir().unwrap();
    let p = outlines(
        d.path(),
        &[
            (11, "PP", deg_sq(0.0, 0.0, 1.0, 1.0)),
            (12, "PP", deg_sq(1.0, 0.0, 2.0, 1.0)),
            (13, "PP", deg_sq(10.0, 0.0, 11.0, 1.0)),
        ],
    );
    let land = vec![bx(0.0, 0.0, 1.0, 1.0), bx(1.0, 0.0, 2.0, 1.0), bx(10.0, 0.0, 11.0, 1.0)];
    // The place's zone in two stretches: round p1 and p2, and round p3.
    let zone = [bx(-1.0, -1.0, 3.0, 2.0), bx(9.0, -1.0, 12.0, 2.0)].concat();
    let data = Datasets::from_parts(HashMap::from([(9, zone)]), HashMap::new(), HashMap::new(), land, "fixture");
    let w = World { o: crate::outlines::Outlines::open(&p).unwrap(), data, _d: d };
    let m = |id: &str, rel: u64| format!("id = \"{id}\"\nname = \"{id}\"\noutline = [\"osm:{rel}\"]\nwater = []\n");
    let (p1, p2, p3) = (m("p1", 11), m("p2", 12), m("p3", 13));
    let pw =
        "id = \"p-waters\"\nname = \"P waters\"\nwater = [{ minus = [\"eez:9\", \"group:p\"] }]\nhand_out = \"p\"\n";
    let out = run(&w, &[PINS, ("p/p1.toml", &p1), ("p/p2.toml", &p2), ("p/p3.toml", &p3), ("p/p-waters.toml", pw)]);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let at = |id: &str, x: f64, y: f64| area_of(&out.resolved[id].geometry).contains([e7(x), e7(y)]);
    assert!(at("p3", 11.5, 0.5) && at("p3", 10.5, 0.5) && !at("p-waters", 11.5, 0.5));
    assert!(at("p-waters", 2.5, 0.5) && at("p-waters", -0.5, 0.5) && !at("p1", -0.5, 0.5));
    assert!(at("p1", 0.5, 0.5) && !at("p1", 2.5, 0.5));
}

/// Cutting a region back to the land never hands one operation a continent's coast: a mask (the
/// land in a box) is only made for a box of 2° or less holding at most `MASK_POINTS` points of the
/// region, and none made holds more than `MASK_POINTS` unless its box is a sixteenth of a degree; the
/// result is the plain intersection's.
#[test]
fn the_land_is_cut_in_bounded_pieces() {
    // Land: 80 × 80 islands of 64 points each (410k points) over 16° × 16°.
    let mut land: Vec<Vec<geom::Ring>> = Vec::new();
    for i in 0..80 {
        for j in 0..80 {
            let (x, y) = (i as f64 * 0.2, j as f64 * 0.2);
            let ring: geom::Ring = (0..64)
                .map(|k| {
                    let t = k as f64 / 64.0 * std::f64::consts::TAU;
                    [e7(x + 0.1 + 0.05 * t.cos()), e7(y + 0.1 + 0.05 * t.sin())]
                })
                .collect();
            land.push(vec![ring]);
        }
    }
    let data = Datasets::from_parts(HashMap::new(), HashMap::new(), HashMap::new(), land, "fixture");
    let region = Area::from_rings(&bx(-1.0, -1.0, 17.0, 17.0));
    let biggest = std::sync::Mutex::new((0usize, [0i64; 4]));
    let mask = |b: [i64; 4]| {
        let m = data.land_in(b);
        let mut g = biggest.lock().unwrap();
        if geom::points(&m) > g.0 {
            *g = (geom::points(&m), b);
        }
        m
    };
    let got = geom::with_mask(&region, &mask, true);
    let (most, b) = *biggest.lock().unwrap();
    assert!(b[2] - b[0] <= 20_000_000 && b[3] - b[1] <= 20_000_000, "{b:?}");
    assert!(most <= geom::MASK_POINTS, "{most}");
    let all = data.land_in([-10_000_000, -10_000_000, 170_000_000, 170_000_000]);
    assert!((geom::area_km2(&got) - geom::area_km2(&all)).abs() < 1.0);
    assert!(geom::area_km2(&got.minus(&all)) < 0.01 && geom::area_km2(&all.minus(&got)) < 0.01);
}
