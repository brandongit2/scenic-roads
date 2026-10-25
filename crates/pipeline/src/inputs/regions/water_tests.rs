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
    assert!(tiles::parse_rules("cover = \"earth\"\n").is_ok());
    assert!(tiles::parse_rules("cover = \"mars\"\n").is_err());
    assert!(tiles::parse_rules("covers = \"earth\"\n").is_err());
}

#[test]
fn nearest_splits_at_the_median_line() {
    let within = Area::from_rings(&bx(-1.0, -1.0, 4.0, 2.0));
    let a = Area::from_rings(&bx(0.0, 0.0, 1.0, 1.0));
    let b = Area::from_rings(&bx(2.0, 0.0, 3.0, 1.0));
    let n = geom::nearest(&within, &[a.clone()], &[b.clone()], 1000.0);
    assert!(n.contains([e7(1.45), e7(0.5)]) && n.contains([e7(-0.5), e7(1.5)]));
    assert!(!n.contains([e7(1.55), e7(0.5)]) && !n.contains([e7(3.5), e7(0.5)]));
    // Within a step of the median (x = 1.5).
    assert!(n.contains([e7(1.49), e7(0.5)]) && !n.contains([e7(1.51), e7(0.5)]));
    // Its complement and it make the whole, with no overlap.
    let m = geom::nearest(&within, &[b.clone()], &[a.clone()], 1000.0);
    assert!(n.intersect(&m).is_empty() || geom::area_km2(&n.intersect(&m)) < 1.0);
    assert!((geom::area_km2(&n.union(&m)) - geom::area_km2(&within)).abs() < 1.0);
    // Nothing to be nearer than: all; nothing to be near: none.
    assert_eq!(geom::nearest(&within, &[a.clone()], &[], 1000.0), within);
    assert!(geom::nearest(&within, &[], &[b.clone()], 1000.0).is_empty());
}

#[test]
fn nearest_splits_of_touching_lands_dont_overlap() {
    // Two lands sharing a border: its points are both's, and only one side wins them.
    let within = Area::from_rings(&bx(-1.0, -1.0, 3.0, 2.0));
    let a = Area::from_rings(&bx(0.0, 0.0, 1.0, 1.0));
    let b = Area::from_rings(&bx(1.0, 0.0, 2.0, 1.0));
    let n = geom::nearest(&within, &[a.clone()], &[b.clone()], 1000.0);
    let m = geom::nearest(&within, &[b.clone()], &[a.clone()], 1000.0);
    let both = geom::area_km2(&n.intersect(&m));
    assert!(both < 1.0, "the splits overlap by {both} km²");
    assert!((geom::area_km2(&n.union(&m)) - geom::area_km2(&within)).abs() < 1.0);
}

#[test]
fn nearest_splits_among_three_touching_lands_cover_once() {
    // Three lands sharing borders (as Ukraine, Crimea and Sevastopol): each one's split against the
    // other two, together, the whole, once.
    let within = Area::from_rings(&bx(-1.0, -1.0, 4.0, 2.0));
    let lands = [
        Area::from_rings(&bx(0.0, 0.0, 1.0, 1.0)),
        Area::from_rings(&bx(1.0, 0.0, 2.0, 1.0)),
        Area::from_rings(&bx(2.0, 0.0, 3.0, 1.0)),
    ];
    let splits: Vec<Area> = (0..3)
        .map(|i| {
            let others: Vec<Area> = (0..3).filter(|&j| j != i).map(|j| lands[j].clone()).collect();
            geom::nearest(&within, &[lands[i].clone()], &others, 1000.0)
        })
        .collect();
    for i in 0..3 {
        for j in i + 1..3 {
            let both = geom::area_km2(&splits[i].intersect(&splits[j]));
            assert!(both < 1.0, "{i} and {j} overlap by {both} km²");
        }
    }
    let sum: f64 = splits.iter().map(geom::area_km2).sum();
    assert!((sum - geom::area_km2(&within)).abs() < 1.0, "{sum} of {}", geom::area_km2(&within));
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
    world_with(1.002)
}

/// The small world, A's land reaching `north` (its outline to 1°: a strip of land in no outline).
fn world_with(north: f64) -> World {
    world_more(north, Vec::new())
}

/// The world with more land.
fn world_more(north: f64, more: Vec<Vec<geom::Ring>>) -> World {
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
        vec![bx(0.0, 0.0, 1.2, north), bx(2.0, 0.0, 3.0, 1.0), bx(1.6, 1.4, 1.62, 1.42), bx(20.0, 20.0, 21.0, 21.0)];
    let land = [land, more].concat();
    let zone1 = vec![bx(-1.0, -1.0, 4.0, 2.5)[0].clone()];
    let eez = HashMap::from([(1, zone1), (2, bx(5.0, 5.0, 6.0, 6.0))]);
    let lakes = HashMap::from([(77, bx(0.2, 0.2, 0.4, 0.4)), (78, bx(0.6, 0.6, 0.61, 0.61))]);
    let data = Datasets::from_parts(eez, HashMap::new(), HashMap::new(), lakes, land, "fixture");
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
water = []
"#;
const DISPUTED: &str = "id = \"disputed\"\nname = \"Disputed zone\"\nwater = [\"eez:2\"]\n";
const FAR: &str =
    "id = \"far\"\nname = \"Far\"\noutline = [\"osm:4\"]\nwater = [{ buffer = \"osm:4\", by = \"12 nmi\" }]\n";
const HIGH: &str = "id = \"high-seas\"\nname = \"High seas\"\nwater = [\"rest\"]\n";
const PINS: (&str, &str) = ("_pins.toml", "osm = \"2026-09-28\"\n");
const RULES: (&str, &str) = ("_rules.toml", "cover = \"earth\"\n");

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
    assert_eq!(owner(0.9, 1.001), vec!["a"], "the strip no outline has");
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
    // Land and water apart (#468): only a region's land is the tile jobs' target (`meets_rect`); its
    // water, like a region with no land, the depth's (`meets_rect_depth`), though still held
    // (`contains`: a bridge's or a pier's ways at sea stay the region's).
    let cov = read::coverage_of(&out.resolved.values().map(|r| r.geometry.clone()).collect::<Vec<_>>());
    let p = |x: f64, y: f64| [e7(x) as i32, e7(y) as i32];
    let bx = |x: f64, y: f64| [p(x, y)[0], p(x, y)[1], p(x, y)[0] + 1, p(x, y)[1] + 1];
    let target = |x: f64, y: f64| cov.meets_rect(bx(x, y));
    let depth = |x: f64, y: f64| cov.meets_rect_depth(bx(x, y));
    assert!(target(0.9, 0.9) && cov.contains(p(0.9, 0.9)), "A's land");
    assert!(!target(-0.5, -0.5) && depth(-0.5, -0.5) && cov.contains(p(-0.5, -0.5)), "A's sea");
    assert!(!target(0.3, 0.3) && depth(0.3, 0.3) && cov.contains(p(0.3, 0.3)), "A's big lake");
    assert!(!target(21.1, 20.5) && depth(21.1, 20.5), "the far land's 12 nmi");
    assert!(!target(5.5, 5.5) && !target(50.0, 50.0) && depth(50.0, 50.0));
    assert!(!cov.contains(p(50.0, 50.0)), "the high seas: no land, the depth's alone");
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
    // Without _rules.toml nothing is checked: old and new files side by side.
    let files: Vec<_> = all().into_iter().filter(|f| f.0 != "high-seas.toml" && f.0 != "_rules.toml").collect();
    let out = run(&w, &files);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
}

#[test]
fn old_files_resolve_as_before_beside_new_ones() {
    let w = world();
    let old = "id = \"old\"\nname = \"Old\"\noutline = [\"osm:4\"]\n";
    let alone = run(&w, &[PINS, ("old.toml", old)]);
    let mut files = all();
    files.retain(|f| f.0 != "far.toml" && f.0 != "_rules.toml");
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

/// Land in no region over `sliver_km2` isn't filled silently: a finding.
#[test]
fn a_large_piece_of_land_in_no_region_is_an_error() {
    // A's land past its outline by 0.05° (about 700 km²).
    let w = world_with(1.05);
    let out = run(&w, &all());
    assert!(
        out.problems
            .iter()
            .any(|p| p.check == "rg-cover" && p.found == vec!["gap"] && p.message.contains("sliver_km2")),
        "{:?}",
        out.problems
    );
}

/// Land in no region cut by a 2° cell's edge is sized whole: two halves each under `sliver_km2`
/// (about 40 km² each side of 2°N) are one piece over it, found at a point of its land.
#[test]
fn land_in_no_region_across_a_cells_edge_is_sized_whole() {
    let w = world_more(1.0, vec![bx(0.46, 1.96, 0.54, 2.04)]);
    let out = run(&w, &all());
    let gaps: Vec<_> = out.problems.iter().filter(|p| p.check == "rg-cover" && p.found == vec!["gap"]).collect();
    assert_eq!(gaps.len(), 1, "{:?}", out.problems);
    let m = &gaps[0].message;
    assert!(m.starts_with("79.") && m.contains("sliver_km2"), "{m}");
    let at: Vec<f64> = m
        .split(" by ")
        .nth(1)
        .unwrap()
        .split('°')
        .take(2)
        .map(|s| s.trim_start_matches(", ").parse().unwrap())
        .collect();
    assert!((0.46..0.54).contains(&at[0]) && (1.96..2.04).contains(&at[1]), "{m}");
    // Under sliver_km2 whole, it's filled: no finding.
    let w = world_more(1.0, vec![bx(0.48, 1.98, 0.52, 2.02)]);
    let out = run(&w, &all());
    assert!(out.problems.is_empty(), "{:?}", out.problems);
}

/// One coastline (#649): land past a region's outline's coast, along it, is its own, with or
/// without `cover`.
#[test]
fn the_coast_is_the_coastlines_land() {
    let w = world();
    let mut files = all();
    files.retain(|f| f.0 != "_rules.toml");
    let out = run(&w, &files);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let mut g = out.resolved["a"].geometry.clone();
    g.shapes.retain(|s| !s.water);
    let land = area_of(&g);
    assert!(land.contains([e7(0.6), e7(1.001)]) && !land.contains([e7(0.6), e7(1.003)]));
    // Its sea: water parts stopping short of the coast (a notch of about 24 km² at the land's
    // east edge) reach it; a notch off the coast stays out.
    let s = "id = \"s\"\nname = \"S\"\noutline = [\"osm:4\"]\n\
             water = [{ minus = [\"box:19.0,19.0,22.0,22.0\", \"box:21.0,20.4,21.02,20.5\", \"box:21.5,20.4,21.52,20.5\"] }]\n";
    let out = run(&w, &[PINS, ("s.toml", s)]);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let mut g = out.resolved["s"].geometry.clone();
    g.shapes.retain(|s| s.water);
    let water = area_of(&g);
    assert!(water.contains([e7(21.01), e7(20.45)]) && !water.contains([e7(21.51), e7(20.45)]));
}

/// The coast is the coastline's, not a cell's edge (#649 review): land in no region inland, at a
/// cell's edge (32°) with land on past it, stays out, though a region with water borders it.
#[test]
fn a_cells_edge_is_no_coast() {
    let w = world_more(1.002, vec![bx(30.0, 30.0, 34.0, 32.0)]);
    let r = "id = \"r\"\nname = \"R\"\n\
             outline = [{ minus = [\"box:30.0,30.0,32.0,32.0\", \"box:31.99,30.5,32.0,30.6\"] }]\n\
             water = [\"box:29.0,29.0,30.0,30.0\"]\n";
    let l = "id = \"l\"\nname = \"L\"\noutline = [\"box:32.0,30.0,34.0,32.0\"]\n";
    let out = run(&w, &[PINS, ("r.toml", r), ("l.toml", l)]);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let land = area_of(&out.resolved["r"].geometry);
    assert!(land.contains([e7(31.9), e7(30.55)]) && !land.contains([e7(31.995), e7(30.55)]));
}

/// The new rules leave regions without water as they resolve alone: the cover's fills and a
/// hand-out never change them (a hand-out naming one says so).
#[test]
fn the_new_rules_never_change_an_old_region() {
    let w = world();
    let old = "id = \"old\"\nname = \"Old\"\noutline = [\"osm:4\"]\n";
    let alone = run(&w, &[PINS, ("old.toml", old)]);
    let mut files = all();
    files.retain(|f| f.0 != "far.toml");
    files.push(("old.toml", old));
    let mixed = run(&w, &files);
    assert_eq!(alone.resolved["old"], mixed.resolved["old"]);
    assert_eq!(mixed.resolved["old"].geometry.shapes[0].tolerance_m, 1000);
    // A hand-out naming an old region: it gets nothing, and the finding says why.
    let ho = "id = \"ho\"\nname = \"Ho\"\nwater = [{ minus = [\"eez:2\"] }]\nhand_out = \"g\"\n";
    let out = run(&w, &[PINS, ("g/old.toml", old), ("g/ho.toml", ho)]);
    assert!(
        out.problems.iter().any(|p| p.check == "rg-ref" && p.message.contains("without water")),
        "{:?}",
        out.problems
    );
    assert_eq!(alone.resolved["old"], out.resolved["old"]);
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
    let data = Datasets::from_parts(
        HashMap::from([(9, zone)]),
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        land,
        "fixture",
    );
    let w = World { o: crate::outlines::Outlines::open(&p).unwrap(), data, _d: d };
    let m = |id: &str, rel: u64| format!("id = \"{id}\"\nname = \"{id}\"\noutline = [\"osm:{rel}\"]\nwater = []\n");
    let (p1, p2, p3) = (m("p1", 11), m("p2", 12), m("p3", 13));
    let pw =
        "id = \"p-waters\"\nname = \"P waters\"\nwater = [{ minus = [\"eez:9\", \"group:p\"] }]\nhand_out = \"p\"\n";
    let out = run(&w, &[PINS, ("p/p1.toml", &p1), ("p/p2.toml", &p2), ("p/p3.toml", &p3), ("p/p-waters.toml", pw)]);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let at = |id: &str, x: f64, y: f64| area_of(&out.resolved[id].geometry).contains([e7(x), e7(y)]);
    // (The stretch round p3 crosses the 2° cells' edge at 10°: handed out whole, both sides.)
    assert!(at("p3", 11.5, 0.5) && at("p3", 10.5, 0.5) && at("p3", 9.5, 0.5) && !at("p-waters", 9.5, 0.5));
    assert!(at("p-waters", 2.5, 0.5) && at("p-waters", 1.5, 1.5), "the shared stretch stays, across the edge at 2°");
    assert!(at("p-waters", 2.5, 0.5) && at("p-waters", -0.5, 0.5) && !at("p1", -0.5, 0.5));
    assert!(at("p1", 0.5, 0.5) && !at("p1", 2.5, 0.5));
}

/// Rings cut into cells by clipping: the cells' pieces make the same area as the rings, none
/// outside its cell; a ring round a whole cell gives the cell; a polygon with a hole keeps it.
#[test]
fn rings_are_cut_into_cells() {
    use super::tiles::{cell_box, TArea, CELL};
    // A ring over 5° × 3° (several cells), with a hole across a cell line.
    let outer = bx(-1.3, 0.7, 3.9, 3.6)[0].clone();
    let hole = bx(0.5, 1.5, 2.5, 2.5)[0].clone();
    let rings = vec![outer, hole];
    let whole = Area::from_rings(&rings);
    let t = TArea::from_rings(&rings);
    assert!(t.cells.len() > 4);
    for (c, a) in &t.cells {
        let b = cell_box(*c);
        assert!(geom::area_km2(&a.minus(&geom::box_area(b))) < 1e-6, "{c:?}");
    }
    assert!((t.km2() - geom::area_km2(&whole)).abs() < 0.01, "{} {}", t.km2(), geom::area_km2(&whole));
    assert!(!t.get((0, 0)).contains([e7(1.5), e7(2.0)]) && t.get((0, 0)).contains([e7(0.3), e7(1.0)]));
    // A ring wholly round a cell: the cell.
    let big = TArea::from_rings(&bx(-5.0, -5.0, 5.0, 5.0));
    assert_eq!(geom::area_km2(&big.get((0, 0))).round(), geom::area_km2(&geom::box_area(cell_box((0, 0)))).round());
    // Across ±180°: a ring from 179° to 181° lands in the first and last columns.
    let across = TArea::from_rings(&[vec![[e7(179.0), 0], [e7(181.0), 0], [e7(181.0), e7(1.0)], [e7(179.0), e7(1.0)]]]);
    assert!(across.cells.contains_key(&(89, 0)) && across.cells.contains_key(&(-90, 0)));
    let _ = CELL;
}

/// What is drawn of an area kept by cell has no cell edges inside it: a ring over several cells
/// with a hole across a cell line is one polygon with its hole, no vertex on a cell line but its
/// own corners.
#[test]
fn the_drawing_has_no_cell_edges_inside() {
    use super::tiles::{dissolved, TArea};
    let t = TArea::from_rings(&[bx(-1.3, 0.7, 3.9, 3.6)[0].clone(), bx(0.5, 1.5, 2.5, 2.5)[0].clone()]);
    assert!(t.cells.len() > 4);
    let d = dissolved(&t);
    assert_eq!(d.polys.len(), 1);
    assert_eq!(d.polys[0].len(), 2);
    assert!((geom::area_km2(&d) - t.km2()).abs() < 0.01);
    let on_cell_line =
        |p: &geom::Pt| p[0].rem_euclid(super::tiles::CELL) == 0 || p[1].rem_euclid(super::tiles::CELL) == 0;
    assert!(!d.polys[0][0].iter().any(on_cell_line), "{:?}", d.polys[0][0]);
}

/// A zone's internal waters overlapping it are unioned with it, never cancelled; and features of
/// one id are unioned as they're read.
#[test]
fn a_zone_and_its_overlapping_internal_waters_are_unioned() {
    let d = tempfile::tempdir().unwrap();
    let p = outlines(d.path(), &[(4, "DD", deg_sq(20.0, 20.0, 21.0, 21.0))]);
    let zone = bx(30.0, 30.0, 32.0, 32.0);
    let iw = bx(31.0, 31.0, 33.0, 33.0);
    let data = Datasets::from_parts(
        HashMap::from([(7, zone.clone())]),
        HashMap::from([(7, iw.clone())]),
        HashMap::new(),
        HashMap::new(),
        vec![bx(20.0, 20.0, 21.0, 21.0)],
        "fixture",
    );
    let w = World { o: crate::outlines::Outlines::open(&p).unwrap(), data, _d: d };
    let z = "id = \"z\"\nname = \"Z\"\nwater = [\"eez:7\"]\n";
    let out = run(&w, &[PINS, ("z.toml", z)]);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let got = geom::area_km2(&area_of(&out.resolved["z"].geometry));
    let want = geom::area_km2(&Area::from_rings(&zone).union(&Area::from_rings(&iw)));
    assert!((got - want).abs() < 1.0, "{got} {want}");
    // Two features of one id, overlapping: their union.
    let gj = br#"{"type":"FeatureCollection","features":[
        {"type":"Feature","properties":{"m":5},"geometry":{"type":"Polygon","coordinates":[[[0,0],[2,0],[2,2],[0,2],[0,0]]]}},
        {"type":"Feature","properties":{"m":5},"geometry":{"type":"Polygon","coordinates":[[[1,1],[3,1],[3,3],[1,3],[1,1]]]}}]}"#;
    let m = super::data::parse_geojson(gj, "m").unwrap();
    let a = Area::from_rings(&m[&5]);
    assert!(a.contains([e7(1.5), e7(1.5)]) && a.contains([e7(2.5), e7(2.5)]) && a.contains([e7(0.5), e7(0.5)]));
}

/// Neighbouring zones' shared border, wiggling under the simplifying tolerance: kept the same on
/// both sides, so they neither overlap nor leave a sliver.
#[test]
fn zones_sharing_a_border_keep_it_the_same() {
    // A border from (1, 0) to (1, 1), wandering up to 30 m (0.00027°) either way every 0.001°: about
    // the simplifying tolerance, so each side simplified alone would keep different points.
    let mut x = 1u64;
    let border: Vec<[f64; 2]> = (0..=1000)
        .map(|i| {
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            let off = if i == 0 || i == 1000 { 0.0 } else { ((x >> 33) as f64 / (1u64 << 31) as f64 - 0.5) * 0.000_54 };
            [1.0 + off, i as f64 / 1000.0]
        })
        .collect();
    let ring = |x0: f64, rev: bool| {
        let mut b = border.clone();
        if rev {
            b.reverse();
        }
        let mut r = vec![[x0, if rev { 1.0 } else { 0.0 }]];
        r.extend(b);
        // (One ring starts mid-border: simplified alone, its sides would split the border at
        // different points.)
        if !rev {
            r.rotate_left(377);
        }
        r.push(r[0]);
        serde_json::to_string(&r).unwrap()
    };
    let gj = format!(
        r#"{{"type":"FeatureCollection","features":[
        {{"type":"Feature","properties":{{"m":1}},"geometry":{{"type":"Polygon","coordinates":[{}]}}}},
        {{"type":"Feature","properties":{{"m":2}},"geometry":{{"type":"Polygon","coordinates":[{}]}}}}]}}"#,
        ring(0.0, false),
        ring(2.0, true)
    );
    let m = super::data::parse_geojson(gj.as_bytes(), "m").unwrap();
    let (a, b) = (Area::from_rings(&m[&1]), Area::from_rings(&m[&2]));
    let both = geom::area_km2(&a.intersect(&b));
    assert!(both < 1e-6, "they overlap by {both} km²");
    // The border's points kept on both sides.
    let pts = |r: &Vec<geom::Ring>| r.iter().flatten().filter(|p| p[1] > 0 && p[1] < e7(1.0)).count();
    assert!(pts(&m[&1]) > 900 && pts(&m[&2]) > 900, "{} {}", pts(&m[&1]), pts(&m[&2]));
}

/// A region without water may not name one with it: held, saying so.
#[test]
fn a_region_without_water_naming_one_with_it_is_held() {
    let w = world();
    let old = "id = \"old\"\nname = \"Old\"\noutline = [\"osm:1\"]\nminus = [\"region:far\"]\n";
    let out = run(&w, &[PINS, ("far.toml", FAR), ("old.toml", old)]);
    assert!(
        out.problems.iter().any(|p| p.check == "rg-ref" && p.about[0] == "old" && p.message.contains("has water")),
        "{:?}",
        out.problems
    );
    assert!(!out.resolved.contains_key("old") && out.resolved.contains_key("far"));
}

/// A region with water whose water names a region the folder hasn't (a coast in a nearest-coast
/// split) is held, never resolved as if that coast were empty.
#[test]
fn water_naming_no_region_is_held() {
    let w = world();
    let out = run(&w, &[PINS, ("a.toml", A), ("isl.toml", ISL), ("slice.toml", SLICE)]);
    assert!(
        out.problems
            .iter()
            .any(|p| p.check == "rg-ref" && p.message.contains("land:b") && p.message.contains("names no region")),
        "{:?}",
        out.problems
    );
    assert!(!out.resolved.contains_key("a"));
}

/// With `cover = "earth"`: a region's piece of water under 50 km², touching none of its land and
/// lying across a 2° cells' edge, joins the region it shares its longest edge with, whole.
#[test]
fn a_sliver_across_a_cells_edge_joins_its_neighbour() {
    let d = tempfile::tempdir().unwrap();
    let p = outlines(d.path(), &[(4, "DD", deg_sq(20.0, 20.0, 21.0, 21.0))]);
    let data = Datasets::from_parts(
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        vec![bx(20.0, 20.0, 21.0, 21.0)],
        "fixture",
    );
    let w = World { o: crate::outlines::Outlines::open(&p).unwrap(), data, _d: d };
    // s: land far away, and a sliver of 4.4 × 5.5 km across the edge at 2°; t: water above it.
    let s = "id = \"s\"\nname = \"S\"\noutline = [\"osm:4\"]\nwater = [\"box:1.98,0.0,2.02,0.05\"]\n";
    let t = "id = \"t\"\nname = \"T\"\nwater = [\"box:1.9,0.05,2.1,0.2\"]\n";
    let out = run(&w, &[PINS, RULES, ("s.toml", s), ("t.toml", t), ("high-seas.toml", HIGH)]);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let at = |id: &str, x: f64, y: f64| area_of(&out.resolved[id].geometry).contains([e7(x), e7(y)]);
    assert!(at("t", 1.99, 0.02) && at("t", 2.01, 0.02), "both sides of the edge to t");
    assert!(!at("s", 1.99, 0.02) && !at("s", 2.01, 0.02) && at("s", 20.5, 20.5));
}

/// Two coasts with the same fingerprint (box, point count, point sums) but different rings: a split
/// and its complement still divide the area exactly (ties decided by the rings, a total order).
#[test]
fn equal_fingerprints_split_exactly() {
    let s = 1_000_000i64;
    let ring = |pts: &[(i64, i64)]| pts.iter().map(|&(x, y)| [x * s, y * s]).collect::<geom::Ring>();
    let a = Area::from_rings(&[ring(&[(0, 0), (4, 0), (3, 1), (4, 4), (0, 4), (1, 3)])]);
    let b = Area::from_rings(&[ring(&[(4, 0), (0, 0), (1, 1), (0, 4), (4, 4), (3, 3)])]);
    let (pa, pb) = (a.polys.iter().flatten().flatten(), b.polys.iter().flatten().flatten());
    assert_eq!(pa.clone().count(), pb.clone().count());
    let sum = |t: (i64, i64), p: &[i64; 2]| (t.0 + p[0], t.1 + p[1]);
    assert_eq!(pa.fold((0, 0), sum), pb.fold((0, 0), sum));
    assert_eq!(a.bbox(), b.bbox());
    assert_ne!(a.polys, b.polys);
    let within = Area::from_rings(&[ring(&[(-6, -6), (10, -6), (10, 10), (-6, 10)])]);
    let one = geom::nearest(&within, &[a.clone()], &[b.clone()], 5_000.0);
    let two = geom::nearest(&within, &[b], &[a], 5_000.0);
    assert!(geom::area_km2(&one.intersect(&two)) < 1e-6, "the two splits overlap");
    let all = geom::area_km2(&within);
    assert!((geom::area_km2(&one.union(&two)) - all).abs() < 1e-6 * all, "the two splits leave a gap");
}

/// A region with water's area read cell by cell (regions-check's report) is its area read whole.
#[test]
fn km2_by_cell_is_the_whole_area() {
    let w = world();
    let out = run(&w, &all());
    assert!(out.resolved.len() > 3);
    // (A region of land and water stores the two as shapes apart: read by cell too.)
    assert!(out.resolved.values().any(|r| r.geometry.shapes.len() == 2 && r.geometry.shapes.iter().any(|s| s.water)));
    for (id, res) in &out.resolved {
        let by_cell = read::km2_of(&res.geometry.shapes, true);
        let whole = read::km2_of(&res.geometry.shapes, false);
        assert!((by_cell - whole).abs() <= 1e-6 * whole.max(1.0), "{id}: {by_cell} against {whole}");
    }
}

/// `hand_out_all`: land inside a place but outside its regions goes to the region sharing the longest
/// border with it, or the nearest when it touches none; the holding region ends empty, unreported.
#[test]
fn hand_out_all_gives_every_stretch() {
    let d = tempfile::tempdir().unwrap();
    let p = outlines(
        d.path(),
        &[
            (21, "PP", deg_sq(0.0, 0.0, 1.0, 1.0)),
            (22, "PP", deg_sq(1.3, 0.3, 2.0, 1.0)),
            (23, "PP", deg_sq(10.0, 0.0, 11.0, 1.0)),
        ],
    );
    // A piece between p1 and p2 (0.6° of edge with p1, 0.3° with p2), and an island near p3.
    let land = vec![
        bx(0.0, 0.0, 1.0, 1.0),
        bx(1.0, 0.0, 1.3, 0.6),
        bx(1.3, 0.3, 2.0, 1.0),
        bx(10.0, 0.0, 11.0, 1.0),
        bx(10.0, 3.0, 10.2, 3.2),
    ];
    let data = Datasets::from_parts(HashMap::new(), HashMap::new(), HashMap::new(), HashMap::new(), land, "fixture");
    let w = World { o: crate::outlines::Outlines::open(&p).unwrap(), data, _d: d };
    let m = |id: &str, rel: u64| format!("id = \"{id}\"\nname = \"{id}\"\noutline = [\"osm:{rel}\"]\nwater = []\n");
    let (p1, p2, p3) = (m("p1", 21), m("p2", 22), m("p3", 23));
    let rest = "id = \"p-rest\"\nname = \"P rest\"\noutline = [{ minus = [\"box:0,0,11,4\", \"osm:21\", \"osm:22\", \"osm:23\"] }]\nwater = []\nhand_out = \"p\"\nhand_out_all = true\n";
    let out = run(&w, &[PINS, ("p/p1.toml", &p1), ("p/p2.toml", &p2), ("p/p3.toml", &p3), ("p/p-rest.toml", rest)]);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let at = |id: &str, x: f64, y: f64| area_of(&out.resolved[id].geometry).contains([e7(x), e7(y)]);
    assert!(at("p1", 1.15, 0.3) && !at("p2", 1.15, 0.3), "the longest border wins");
    assert!(at("p3", 10.1, 3.1), "an island touching none goes to the nearest");
    assert!(!out.resolved.contains_key("p-rest"));
}

/// `hand_out_all` across ±180°: an island just east of the antimeridian goes to the member just west
/// of it, not to a farther one on its own side.
#[test]
fn hand_out_all_looks_across_the_antimeridian() {
    let d = tempfile::tempdir().unwrap();
    let p = outlines(
        d.path(),
        &[(31, "PP", deg_sq(179.0, 0.0, 179.95, 1.0)), (32, "PP", deg_sq(-175.5, 0.0, -175.0, 1.0))],
    );
    let land = vec![bx(179.0, 0.0, 179.95, 1.0), bx(-175.5, 0.0, -175.0, 1.0), bx(-179.9, 0.2, -179.7, 0.4)];
    let data = Datasets::from_parts(HashMap::new(), HashMap::new(), HashMap::new(), HashMap::new(), land, "fixture");
    let w = World { o: crate::outlines::Outlines::open(&p).unwrap(), data, _d: d };
    let m = |id: &str, rel: u64| format!("id = \"{id}\"\nname = \"{id}\"\noutline = [\"osm:{rel}\"]\nwater = []\n");
    let (west, east) = (m("west", 31), m("east", 32));
    let rest = "id = \"p-rest\"\nname = \"P rest\"\noutline = [\"box:-180,0,-179,1\"]\nwater = []\nhand_out = \"p\"\nhand_out_all = true\n";
    let out = run(&w, &[PINS, ("p/west.toml", &west), ("p/east.toml", &east), ("p/p-rest.toml", rest)]);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let at = |id: &str, x: f64, y: f64| area_of(&out.resolved[id].geometry).contains([e7(x), e7(y)]);
    assert!(at("west", -179.8, 0.3), "the nearer member, across ±180°");
    assert!(!at("east", -179.8, 0.3));
    assert!(!out.resolved.contains_key("p-rest"));
}

/// `hand_out_all` with no member able to take a stretch (none has land): the leftover is a
/// finding, never stored quietly as a region.
#[test]
fn a_hand_out_all_leftover_is_a_finding() {
    let d = tempfile::tempdir().unwrap();
    let p = outlines(d.path(), &[(41, "PP", deg_sq(5.0, 5.0, 6.0, 6.0))]);
    let land = vec![bx(0.0, 0.0, 0.5, 0.5)];
    let data = Datasets::from_parts(HashMap::new(), HashMap::new(), HashMap::new(), HashMap::new(), land, "fixture");
    let w = World { o: crate::outlines::Outlines::open(&p).unwrap(), data, _d: d };
    let sea = "id = \"q1\"\nname = \"Q1\"\noutline = [\"osm:41\"]\nwater = [\"box:5,5,6,6\"]\n";
    let rest = "id = \"q-rest\"\nname = \"Q rest\"\noutline = [\"box:0,0,1,1\"]\nwater = []\nhand_out = \"q\"\nhand_out_all = true\n";
    let out = run(&w, &[PINS, ("q/q1.toml", sea), ("q/q-rest.toml", rest)]);
    assert!(
        out.problems.iter().any(|p| p.check == "rg-hand-out" && p.message.contains("q-rest")),
        "{:?}",
        out.problems
    );
    assert!(!out.resolved.contains_key("q-rest"));
}

/// A region with water whose outline names a relation the pass's outline set and extract lack
/// (spain/perejil-island.toml, #487): held by rg-extract, and the relation named in the needs, as
/// for a region without water, so the held report lists it and the lead plans the extract.
#[test]
fn a_region_with_water_missing_from_the_extract_names_its_needs() {
    let w = world();
    let islet = "id = \"islet\"\nname = \"Islet\"\noutline = [\"osm:21030917\"]\nwater = [\"eez:2\"]\n";
    let out = run(&w, &[PINS, ("spain/islet.toml", islet)]);
    assert!(out.problems.iter().any(|p| p.check == "rg-extract" && p.about[0] == "islet"), "{:?}", out.problems);
    assert!(!out.resolved.contains_key("islet"));
    assert_eq!(out.needs.iter().map(|i| i.to_string()).collect::<Vec<_>>(), vec!["r21030917"]);
    // Disabled, it still needs them: it's checked like any region (the extract is the check's,
    // not a build's), so it isn't held for good.
    let off = format!("{islet}disabled = true\n");
    let out = run(&w, &[PINS, ("spain/islet.toml", &off)]);
    assert_eq!(out.needs.iter().map(|i| i.to_string()).collect::<Vec<_>>(), vec!["r21030917"]);
}

#[test]
fn disabled_regions_still_cover_the_earth_once() {
    // The island and the far land disabled: the same geometry, byte for byte, and the Earth once
    // (the high seas, the rest, still less what they cover).
    let w = world();
    let on = run(&w, &all());
    let isl = format!("{ISL}disabled = true\n");
    let far = format!("{FAR}disabled = true\n");
    let files: Vec<(&str, &str)> = all()
        .into_iter()
        .map(|(p, t)| match p {
            "w/isl.toml" => (p, isl.as_str()),
            "far.toml" => (p, far.as_str()),
            _ => (p, t),
        })
        .collect();
    let off = run(&w, &files);
    assert!(off.problems.is_empty(), "{:#?}", off.problems);
    assert_eq!(off.resolved.keys().collect::<Vec<_>>(), on.resolved.keys().collect::<Vec<_>>());
    for (id, r) in &on.resolved {
        assert_eq!(r.geometry.encode(), off.resolved[id].geometry.encode(), "{id}");
    }
    let sum: f64 = off.resolved.values().map(|r| geom::area_km2(&area_of(&r.geometry))).sum();
    assert!((sum - geom::area_km2(&geom::earth())).abs() < 5.0, "{sum}");
}
