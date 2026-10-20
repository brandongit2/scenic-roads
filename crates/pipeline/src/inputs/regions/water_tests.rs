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
