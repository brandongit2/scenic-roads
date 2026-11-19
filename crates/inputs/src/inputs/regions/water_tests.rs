//! Land and water (docs/inputs.md §6.1.3, §6.1.4): the new parts and operations one by one, and a
//! small folder resolved whole: a nearest-coast split, a disputed zone of its own, a 12 nmi ring
//! cut at the median line, a lake, a leftover of land, a land slice and the high seas as the rest,
//! the Earth covered once.

use super::data::Datasets;
use super::file::{parse, Part};
use super::geom::{self, Area};
use super::progress::Step;
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
    // (Nothing is settled by size: no sliver_km2.)
    assert!(tiles::parse_rules("cover = \"earth\"\nsliver_km2 = 50\n").is_err());
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
    // A degree square at the equator: 111.2 km a side on the mean sphere.
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
pub(super) struct World {
    pub(super) _d: tempfile::TempDir,
    pub(super) o: buildstore::outlines::Outlines,
    data: Datasets,
}

pub(super) fn world() -> World {
    world_with(1.002)
}

/// The small world, A's land reaching `north` (its outline to 1°: a strip of land in no outline).
fn world_with(north: f64) -> World {
    world_more(north, Vec::new())
}

/// The world with more land.
fn world_more(north: f64, more: Vec<Vec<geom::Ring>>) -> World {
    world_zones(north, more, HashMap::new())
}

/// The world with more land and more zones.
fn world_zones(north: f64, more: Vec<Vec<geom::Ring>>, zones: HashMap<u64, Vec<geom::Ring>>) -> World {
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
    World {
        o: buildstore::outlines::Outlines::open(&p).unwrap(),
        data: world_data(north, more, zones, "fixture"),
        _d: d,
    }
}

/// The small world's datasets (`world_zones`), named `names`.
pub(super) fn world_data(
    north: f64,
    more: Vec<Vec<geom::Ring>>,
    zones: HashMap<u64, Vec<geom::Ring>>,
    names: &str,
) -> Datasets {
    let land =
        vec![bx(0.0, 0.0, 1.2, north), bx(2.0, 0.0, 3.0, 1.0), bx(1.6, 1.4, 1.62, 1.42), bx(20.0, 20.0, 21.0, 21.0)];
    let land = [land, more].concat();
    let zone1 = vec![bx(-1.0, -1.0, 4.0, 2.5)[0].clone()];
    let mut eez = HashMap::from([(1, zone1), (2, bx(5.0, 5.0, 6.0, 6.0))]);
    eez.extend(zones);
    let lakes = HashMap::from([(77, bx(0.2, 0.2, 0.4, 0.4)), (78, bx(0.6, 0.6, 0.61, 0.61))]);
    Datasets::from_parts(eez, HashMap::new(), HashMap::new(), lakes, land, names)
}

fn run(w: &World, files: &[(&str, &str)]) -> resolve::Outcome {
    run_counted(w, files).0
}

/// `run`, and each step its progress counted (crate::inputs::regions::progress): the step, its
/// items done and its total.
fn run_counted(w: &World, files: &[(&str, &str)]) -> (resolve::Outcome, Vec<(Step, u64, u64)>, f64) {
    let m: BTreeMap<String, Vec<u8>> = files.iter().map(|(p, t)| (p.to_string(), t.as_bytes().to_vec())).collect();
    let folder = resolve::Folder::read(m.keys().cloned(), &|p| Ok(m[p].clone())).unwrap();
    let others = BTreeMap::new();
    let ctx = resolve::Context {
        pass: Some(PASS),
        outlines: Some(&w.o),
        extract: None,
        others: &others,
        memo: None,
        kept: None,
        reuse: None,
        names: String::new(),
        data: Some(&w.data),
        progress: Default::default(),
    };
    let out = resolve::resolve(&folder, &ctx);
    (out, ctx.progress.steps(), ctx.progress.share())
}

/// Task #1145: a resolve counts every step it takes, in order, each to its total: the regions, the
/// cells of every pass (the cover check's every cell of the Earth), the regions stored; so its
/// share ends whole, having gone on through each.
#[test]
fn a_resolve_counts_every_step_to_its_total() {
    let w = world();
    let (out, steps, share) = run_counted(&w, &all());
    assert!(out.problems.is_empty(), "{:#?}", out.problems);
    let taken: Vec<Step> = steps.iter().map(|s| s.0).collect();
    assert_eq!(taken, Step::ALL, "every step taken, in order: the folder has water, cover and a rest");
    for (step, done, total) in &steps {
        assert_eq!(done, total, "{step:?}: {done} of {total}");
    }
    let of = |s: Step| steps.iter().find(|x| x.0 == s).map(|x| x.2);
    let cells = 180 * 90;
    assert_eq!(of(Step::Cover), Some(cells));
    // No region without water (the slice has a water key, if empty); the seven with it, their water
    // but the rest's (what's left at the end), and every region stored.
    assert_eq!((of(Step::Plain), of(Step::Land), of(Step::Water)), (Some(0), Some(7), Some(6)));
    assert_eq!(of(Step::Stored), Some(7));
    assert_eq!(share, 1.0);
}

pub(super) const A: &str = r#"id = "a"
name = "A"
outline = ["osm:1"]
water = [{ minus = [{ nearest = "eez:1", to = ["land:a"], than = ["land:b", "land:slice"] }, "region:isl"] }]
"#;
pub(super) const B: &str = r#"id = "b"
name = "B"
outline = ["osm:2"]
minus = ["region:slice"]
water = [{ minus = [{ nearest = "eez:1", to = ["land:b", "land:slice"], than = ["land:a"] }, "region:isl"] }]
"#;
pub(super) const ISL: &str = r#"id = "isl"
name = "Island"
outline = ["osm:3"]
water = [{ nearest = { intersect = [{ buffer = "osm:3", by = "12 nmi" }, "eez:1"] }, to = ["land:isl"], than = ["land:a", "land:b"] }]
"#;
pub(super) const SLICE: &str = r#"id = "slice"
name = "Slice"
outline = [{ intersect = ["osm:2", "box:2.0,0.0,3.0,0.5"] }]
water = []
"#;
pub(super) const DISPUTED: &str = "id = \"disputed\"\nname = \"Disputed zone\"\nwater = [\"eez:2\"]\n";
const FAR: &str =
    "id = \"far\"\nname = \"Far\"\noutline = [\"osm:4\"]\nwater = [{ buffer = \"osm:4\", by = \"12 nmi\" }]\n";
pub(super) const HIGH: &str = "id = \"high-seas\"\nname = \"High seas\"\nwater = [\"rest\"]\n";
pub(super) const PINS: (&str, &str) = ("_pins.toml", "osm = \"2026-09-28\"\n");
const RULES: (&str, &str) = ("_rules.toml", "cover = \"earth\"\n");

pub(super) fn all() -> Vec<(&'static str, &'static str)> {
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

/// Task #897: the regions' resolution paused after every piece kept (each region without water
/// resolved, each with water's land and water), resumed each time, comes to the same as one never
/// paused, each piece made once.
#[test]
fn a_resolution_paused_at_every_piece_resumes_to_the_same() {
    let w = world();
    let old = "id = \"old\"\nname = \"Old\"\noutline = [\"osm:4\"]\n";
    let mut files = all();
    files.retain(|f| f.0 != "far.toml" && f.0 != "_rules.toml");
    files.push(("old.toml", old));
    // (Old's resolution and the regions with water's lands and waters: 12 pieces, a run keeping
    // one each, as the last run, keeping none, didn't stop.)
    assert_eq!(paused_at_every_piece(&w, &files), 12, "a stop after each piece");
}

/// A folder's remainder is a piece of its own (#1448 review): paused after it, a check takes it
/// back, never makes it again.
#[test]
fn a_remainder_is_kept_as_a_piece() {
    let w = world_more(1.002, vec![bx(30.0, 20.0, 33.0, 21.0)]);
    let l = "id = \"l\"\nname = \"L\"\noutline = [\"box:29.9,19.9,31.0,21.1\"]\nwater = []\n";
    let r = "id = \"r\"\nname = \"R\"\noutline = [\"box:32.0,19.9,33.1,21.1\"]\nwater = []\n";
    let rem = "of = \"box:30.0,20.0,33.0,21.0\"\n";
    let mut files = all();
    files.retain(|f| f.0 != "far.toml" && f.0 != "_rules.toml");
    let without = paused_at_every_piece(&w, &files);
    files.extend([("p/l.toml", l), ("p/r.toml", r)]);
    let lands = paused_at_every_piece(&w, &files);
    files.push(("p/_remainder.toml", rem));
    // (Each of l and r: its land and its water; then the remainder, one piece.)
    assert_eq!(lands, without + 4);
    assert_eq!(paused_at_every_piece(&w, &files), lands + 1, "the remainder one piece");
}

/// Folder `files` resolved, then again paused after every piece kept and resumed each time: the
/// same outcome as one never paused; how many pauses it took.
fn paused_at_every_piece(w: &World, files: &[(&str, &str)]) -> usize {
    use jobrt::resume_log::STOP_AFTER;
    let m: BTreeMap<String, Vec<u8>> = files.iter().map(|(p, t)| (p.to_string(), t.as_bytes().to_vec())).collect();
    let folder = resolve::Folder::read(m.keys().cloned(), &|p| Ok(m[p].clone())).unwrap();
    let others = BTreeMap::new();
    let with = |kept: Option<&super::kept::Kept>| {
        let ctx = resolve::Context {
            pass: Some(PASS),
            outlines: Some(&w.o),
            extract: None,
            others: &others,
            memo: None,
            kept,
            reuse: None,
            names: String::new(),
            data: Some(&w.data),
            progress: Default::default(),
        };
        resolve::resolve(&folder, &ctx)
    };
    let fresh = with(None);
    assert!(fresh.problems.is_empty(), "{:?}", fresh.problems);
    let d = tempfile::tempdir().unwrap();
    let mut stops = 0;
    let resumed = loop {
        let kept = super::kept::Kept::default();
        STOP_AFTER.set(Some(1));
        kept.open(d.path(), "a", true);
        let out = with(Some(&kept));
        match kept.stopped() {
            Err(e) => {
                assert!(jobrt::control::is_paused(&e), "{e:#}");
                stops += 1;
            }
            Ok(()) => break out,
        }
    };
    assert_eq!(resumed.resolved, fresh.resolved);
    assert_eq!(resumed.problems, fresh.problems);
    assert_eq!(resumed.needs, fresh.needs);
    assert_eq!(resumed.keys, fresh.keys);
    stops
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
        kept: None,
        reuse: None,
        names: String::new(),
        data: None,
        progress: Default::default(),
    };
    let out = resolve::resolve(&folder, &ctx);
    assert!(out.problems.iter().any(|p| p.check == "rg-data"), "{:?}", out.problems);
}

/// Land in no region is a finding, at a point of its land, naming the regions round it; nothing
/// is given to them (task #1427).
#[test]
fn a_large_piece_of_land_in_no_region_is_an_error() {
    // A's land past its outline by 0.05° (about 700 km²).
    let w = world_with(1.05);
    let out = run(&w, &all());
    let gaps: Vec<_> = out.problems.iter().filter(|p| p.check == "rg-cover" && p.found == vec!["gap"]).collect();
    assert_eq!(gaps.len(), 1, "{:?}", out.problems);
    assert!(gaps[0].message.contains("of it land") && gaps[0].message.contains("around it a ("), "{}", gaps[0].message);
    assert!(!area_of(&out.resolved["a"].geometry).contains([e7(0.6), e7(1.03)]), "not filled");
}

/// The place a finding names, in degrees.
fn at_of(message: &str) -> Vec<f64> {
    message
        .split(" by ")
        .nth(1)
        .unwrap()
        .split('°')
        .take(2)
        .map(|s| s.trim_start_matches(", ").parse().unwrap())
        .collect()
}

/// Land in no region cut by a 2° cell's edge is sized whole: two halves (about 40 km² each side of
/// 2°N) are one piece, found at a point of its land; a small one too (task #1427: no size is
/// settled).
#[test]
fn land_in_no_region_across_a_cells_edge_is_sized_whole() {
    let w = world_more(1.0, vec![bx(0.46, 1.96, 0.54, 2.04)]);
    let out = run(&w, &all());
    let gaps: Vec<_> = out.problems.iter().filter(|p| p.check == "rg-cover" && p.found == vec!["gap"]).collect();
    assert_eq!(gaps.len(), 1, "{:?}", out.problems);
    let m = &gaps[0].message;
    assert!(m.starts_with("79."), "{m}");
    let at = at_of(m);
    assert!((0.46..0.54).contains(&at[0]) && (1.96..2.04).contains(&at[1]), "{m}");
    let w = world_more(1.0, vec![bx(0.48, 1.98, 0.52, 2.02)]);
    let out = run(&w, &all());
    let gaps: Vec<_> = out.problems.iter().filter(|p| p.check == "rg-cover" && p.found == vec!["gap"]).collect();
    assert_eq!(gaps.len(), 1, "{:?}", out.problems);
    assert!(gaps[0].message.starts_with("19."), "{}", gaps[0].message);
}

/// Land in no region is reported at any size above the coastline's own precision, not under the
/// hair: an island of about 0.012 km² (0.001° a side) in no region is a gap, one of about 0.00012 km²
/// (under a square of the simplification's side) is not (task #1451).
#[test]
fn a_small_island_in_no_region_is_an_error_a_speck_is_not() {
    // (A sea region round it, as an island has: the island itself is what's in no region.)
    let sea = |e: f64| {
        format!(
            "id = \"s\"\nname = \"S\"\nwater = [{{ buffer = \"box:30.0,20.0,{},{}\", by = \"10 km\" }}]\n",
            30.0 + e,
            20.0 + e
        )
    };
    let (big, small) = (sea(0.001), sea(0.0001));
    let w = world_more(1.0, vec![bx(30.0, 20.0, 30.001, 20.001)]);
    let mut files = all();
    files.push(("s.toml", &big));
    let out = run(&w, &files);
    let gaps: Vec<_> = out.problems.iter().filter(|p| p.check == "rg-cover" && p.found == vec!["gap"]).collect();
    assert_eq!(gaps.len(), 1, "{:?}", out.problems);
    let km2 = out.fit.gaps.iter().map(|g| g.land_km2).fold(0.0, f64::max);
    assert!(km2 < tiles::HAIR_KM2 && km2 > tiles::LAND_MIN_KM2, "{km2}");
    let at = at_of(&gaps[0].message);
    assert!((30.0..30.001).contains(&at[0]) && (20.0..20.001).contains(&at[1]), "{}", gaps[0].message);
    let w = world_more(1.0, vec![bx(30.0, 20.0, 30.0001, 20.0001)]);
    let mut files = all();
    files.push(("s.toml", &small));
    let out = run(&w, &files);
    assert!(out.problems.iter().all(|p| p.found != vec!["gap"]), "{:?}", out.problems);
}

/// Land is judged by its pieces (task #1451 review): a strip of land in no region over
/// `LAND_MIN_KM2` in size but under `SIMPLIFY_M / 2` wide (5.5 m) is two sources' lines
/// disagreeing, no finding; the same length 22 m wide is one.
#[test]
fn a_strip_of_land_in_no_region_is_judged_by_its_width() {
    let sea = |n: f64| {
        format!("id = \"s\"\nname = \"S\"\nwater = [{{ buffer = \"box:30.5,20.5,30.51,{n}\", by = \"10 km\" }}]\n")
    };
    for (north, found) in [(20.50005, false), (20.5002, true)] {
        let s = sea(north);
        let w = world_more(1.0, vec![bx(30.5, 20.5, 30.51, north)]);
        let mut files = all();
        files.push(("s.toml", &s));
        let out = run(&w, &files);
        let gaps = out.problems.iter().filter(|p| p.check == "rg-cover" && p.found == vec!["gap"]).count();
        assert_eq!(gaps == 1, found, "{north}: {:?}", out.problems);
        assert!(gaps <= 1, "{:?}", out.problems);
    }
}

/// The same bar for land in two regions: an overlap of two boxes on land 22 m wide (0.0025 km²,
/// under the hair) is an overlap, one 5.5 m wide is not; both said by the largest land piece.
#[test]
fn land_in_two_regions_is_judged_by_its_pieces_too() {
    let a = "id = \"a2\"\nname = \"A2\"\noutline = [\"box:30.4,20.4,30.5005,20.6\"]\nwater = []\n";
    for (east, found) in [(30.50055, false), (30.5003, true)] {
        let b = format!("id = \"b2\"\nname = \"B2\"\noutline = [\"box:{east},20.4,30.6,20.6\"]\nwater = []\n");
        let w = world_more(1.0, vec![bx(30.5, 20.5, 30.501, 20.501)]);
        let mut files = all();
        files.extend([("p/a2.toml", a), ("p/b2.toml", b.as_str())]);
        let out = run(&w, &files);
        let ov: Vec<_> = out.problems.iter().filter(|p| p.check == "rg-cover" && p.found == vec!["overlap"]).collect();
        assert_eq!(ov.len() == 1, found, "{east}: {:?}", out.problems);
        if found {
            let o = &out.fit.overlaps[&("a2".to_string(), "b2".to_string())];
            assert!(o.largest_solid_km2 > tiles::LAND_MIN_KM2 && o.land_km2 + o.water_km2 < tiles::HAIR_KM2, "{o:?}");
            let at = at_of(&ov[0].message);
            assert!((30.5003..30.5005).contains(&at[0]) && (20.5..20.501).contains(&at[1]), "{}", ov[0].message);
        }
    }
}

/// A remainder takes a stretch of free land whole or not at all (task #1451 review): a strip of
/// land 890 m deep between the outlines of three regions and the coast, a remainder over the
/// seaward half of its middle; the inland half, which borders the regions and the remainder's
/// piece and no longer the coastline, still goes to the region beside it, as the whole strip would
/// without the remainder.
#[test]
fn a_remainder_over_the_seaward_half_of_a_coast_strip_leaves_no_land_behind() {
    let w = world_more(1.0, vec![bx(29.9, 20.0, 31.1, 20.508)]);
    let l = "id = \"l\"\nname = \"L\"\noutline = [\"box:29.9,19.9,30.2,20.6\"]\nwater = []\n";
    let m = "id = \"m\"\nname = \"M\"\noutline = [\"box:30.2,19.9,30.8,20.5\"]\nwater = []\n";
    let r = "id = \"r\"\nname = \"R\"\noutline = [\"box:30.8,19.9,31.1,20.6\"]\nwater = []\n";
    let rem = "of = \"box:30.2,20.504,30.8,20.6\"\n";
    let land_left = |files: &[(&str, &str)]| -> f64 {
        let out = run(&w, files);
        out.fit.gaps.iter().map(|g| g.land_km2).sum()
    };
    let mut files = all();
    files.extend([("p/l.toml", l), ("p/m.toml", m), ("p/r.toml", r)]);
    assert!(land_left(&files) < 0.01, "the whole strip goes to the regions beside it, as before");
    files.push(("p/_remainder.toml", rem));
    assert!(land_left(&files) < 0.01, "the inland half too, with the remainder");
}

/// Task #1427: a county missing from a divided place is a gap at a point inside it, naming the
/// counties round it, and none of them takes it; two counties both having a strip is an overlap
/// naming them, each keeping it.
#[test]
fn a_missing_county_is_a_gap_naming_its_neighbours() {
    let w = world_more(1.002, vec![bx(30.0, 20.0, 33.0, 21.0)]);
    let l = "id = \"l\"\nname = \"L\"\noutline = [\"box:29.9,19.9,31.0,21.1\"]\nwater = []\n";
    let r = "id = \"r\"\nname = \"R\"\noutline = [\"box:32.0,19.9,33.1,21.1\"]\nwater = []\n";
    let s = "id = \"s\"\nname = \"S\"\nwater = [{ buffer = \"box:30.0,20.0,33.0,21.0\", by = \"10 km\" }]\n";
    let mut files = all();
    files.extend([("p/l.toml", l), ("p/r.toml", r), ("s.toml", s)]);
    let out = run(&w, &files);
    let gaps: Vec<_> = out.problems.iter().filter(|p| p.check == "rg-cover").collect();
    assert_eq!(gaps.len(), 1, "{:?}", out.problems);
    let m = &gaps[0].message;
    assert_eq!(gaps[0].found, vec!["gap"]);
    let at = at_of(m);
    assert!((31.0..32.0).contains(&at[0]) && (20.0..21.0).contains(&at[1]), "{m}");
    let mut named: Vec<&str> = out
        .fit
        .gaps
        .iter()
        .filter(|g| !g.rest && g.km2 > tiles::HAIR_KM2)
        .flat_map(|g| g.edges.iter().map(|e| e.0.as_str()))
        .collect();
    named.sort();
    assert_eq!(named, ["l", "r", "s"], "{m}");
    assert!(m.contains("around it ") && m.contains(" r (") && m.contains(" s ("), "{m}");
    assert!(gaps[0].files.contains(&"p/l.toml".to_string()) && gaps[0].files.contains(&"p/r.toml".to_string()));
    for id in ["l", "r", "s"] {
        assert!(!area_of(&out.resolved[id].geometry).contains([e7(31.5), e7(20.5)]), "{id}: nothing moved");
    }
    // The middle county's outline overlapping its neighbour by a strip of 0.02° (about 2 km).
    let m = "id = \"m\"\nname = \"M\"\noutline = [\"box:30.98,19.9,32.0,21.0\"]\nwater = []\n";
    let mut files = all();
    files.extend([("p/l.toml", l), ("p/r.toml", r), ("p/m.toml", m), ("s.toml", s)]);
    let out = run(&w, &files);
    let said: Vec<_> = out.problems.iter().filter(|p| p.check == "rg-cover").collect();
    assert_eq!(said.len(), 1, "{:?}", out.problems);
    assert_eq!(
        (said[0].found.clone(), said[0].about.clone()),
        (vec!["overlap".to_string()], vec!["l".to_string(), "m".to_string()])
    );
    let at = at_of(&said[0].message);
    assert!((30.98..31.0).contains(&at[0]), "{}", said[0].message);
    for id in ["l", "m"] {
        assert!(area_of(&out.resolved[id].geometry).contains([e7(30.99), e7(20.5)]), "{id} keeps it");
    }
}

/// Task #1448: a folder's declared remainder. What of its `of` no region's outline takes goes to the
/// folder's regions: by default to the nearest, split at the median line; with `by`, each the pieces
/// inside its official part first, the rest by nearest. What another region takes (one outside the
/// folder) stays its own; nothing overlaps, nothing is left. A `by` naming no region of the folder
/// holds the folder's regions.
#[test]
fn a_folders_remainder_goes_to_its_regions() {
    let w = world_more(1.002, vec![bx(30.0, 20.0, 33.0, 21.0)]);
    let l = "id = \"l\"\nname = \"L\"\noutline = [\"box:29.9,19.9,31.0,21.1\"]\nwater = []\n";
    let r = "id = \"r\"\nname = \"R\"\noutline = [\"box:32.0,19.9,33.1,21.1\"]\nwater = []\n";
    let s = "id = \"s\"\nname = \"S\"\nwater = [{ buffer = \"box:30.0,20.0,33.0,21.0\", by = \"10 km\" }]\n";
    let q = "id = \"q\"\nname = \"Q\"\noutline = [\"box:31.4,20.4,31.6,20.6\"]\nwater = []\n";
    let run_with = |rem: &str| {
        let mut files = all();
        files.extend([("p/l.toml", l), ("p/r.toml", r), ("s.toml", s), ("q.toml", q), ("p/_remainder.toml", rem)]);
        run(&w, &files)
    };
    let in_ =
        |out: &resolve::Outcome, id: &str, x: f64| area_of(&out.resolved[id].geometry).contains([e7(x), e7(20.2)]);
    let median = run_with("of = \"box:30.0,20.0,33.0,21.0\"\n");
    assert!(median.problems.is_empty(), "{:#?}", median.problems);
    assert!(in_(&median, "l", 31.2) && !in_(&median, "r", 31.2));
    assert!(in_(&median, "r", 31.8) && !in_(&median, "l", 31.8));
    assert!(in_(&median, "r", 31.6) && !in_(&median, "l", 31.6), "east of the median line, 31.5°");
    let mid = |out: &resolve::Outcome, id: &str| area_of(&out.resolved[id].geometry).contains([e7(31.5), e7(20.5)]);
    assert!(mid(&median, "q") && !mid(&median, "l") && !mid(&median, "r"), "what q takes stays q's");
    // Official lines: l's to 31.7°, the rest to the nearest.
    let official = run_with("of = \"box:30.0,20.0,33.0,21.0\"\n[by]\nl = \"box:30.0,19.0,31.7,22.0\"\n");
    assert!(official.problems.is_empty(), "{:#?}", official.problems);
    assert!(in_(&official, "l", 31.6) && !in_(&official, "r", 31.6), "l's by its official part");
    assert!(in_(&official, "r", 31.85) && !in_(&official, "l", 31.85));
    assert!(mid(&official, "q") && !mid(&official, "l"));
    // A `by` naming no region of the folder: the folder's regions held, saying why.
    let wrong = run_with("of = \"box:30.0,20.0,33.0,21.0\"\n[by]\nq = \"box:30.0,19.0,31.7,22.0\"\n");
    let p: Vec<_> = wrong.problems.iter().filter(|p| p.check == "rg-ref").collect();
    assert_eq!(p.len(), 1, "{:#?}", wrong.problems);
    assert!(p[0].message.contains("by.q names no region of the folder") && p[0].files == ["p/_remainder.toml"]);
    assert!(!wrong.resolved.contains_key("l") && !wrong.resolved.contains_key("r"));
    // Nested: the inner folder's first (r alone takes 31.8–32°), then the outer's median line runs
    // between l's edge, 31°, and r's land as the inner one left it, 31.8°.
    let mut files = all();
    files.extend([
        ("p/l.toml", l),
        ("p/i/r.toml", r),
        ("s.toml", s),
        ("p/_remainder.toml", "of = \"box:30.0,20.0,33.0,21.0\"\n"),
        ("p/i/_remainder.toml", "of = \"box:31.8,20.0,33.0,21.0\"\n"),
    ]);
    let nested = run(&w, &files);
    assert!(nested.problems.is_empty(), "{:#?}", nested.problems);
    assert!(in_(&nested, "r", 31.45) && !in_(&nested, "l", 31.45), "inner first");
    assert!(in_(&nested, "l", 31.35) && !in_(&nested, "r", 31.35));
    // Its parts are the sources' alone; a bad one is the file's fault.
    assert!(file::parse_remainder("of = \"region:l\"\n").is_err());
    assert!(file::parse_remainder("of = \"osm:1\"\nstep = \"10 m\"\n").is_err());
    assert!(file::parse_remainder("of = \"osm:1\"\nby = { \"L\" = \"osm:2\" }\n").is_err());
    assert!(file::parse_remainder("of = \"osm:1\"\nsplit = \"median\"\n").is_err());
}

/// A remainder its folder's regions can't take as declared holds them all, saying why (#1448
/// review): two `by` parts overlapping in it past HAIR_KM2, a region without water, a held region
/// of the folder, a held region outside it whose outline may reach it; a held region far from it
/// holds nothing.
#[test]
fn a_remainder_that_cant_be_taken_holds_its_folder() {
    let w = world_more(1.002, vec![bx(30.0, 20.0, 33.0, 21.0)]);
    let l = "id = \"l\"\nname = \"L\"\noutline = [\"box:29.9,19.9,31.0,21.1\"]\nwater = []\n";
    let r = "id = \"r\"\nname = \"R\"\noutline = [\"box:32.0,19.9,33.1,21.1\"]\nwater = []\n";
    let s = "id = \"s\"\nname = \"S\"\nwater = [{ buffer = \"box:30.0,20.0,33.0,21.0\", by = \"10 km\" }]\n";
    let median = "of = \"box:30.0,20.0,33.0,21.0\"\n";
    let run_with = |more: &[(&'static str, &'static str)]| {
        let mut files = all();
        files.extend([("p/l.toml", l), ("p/r.toml", r), ("s.toml", s), ("p/_remainder.toml", median)]);
        for (p, t) in more {
            files.retain(|f| f.0 != *p);
            files.push((p, t));
        }
        run(&w, &files)
    };
    let said = |out: &resolve::Outcome, what: &str| -> Vec<resolve::Problem> {
        out.problems.iter().filter(|p| p.check == "rg-ref" && p.message.contains(what)).cloned().collect()
    };
    let folder_held = |out: &resolve::Outcome| !out.resolved.contains_key("l") && !out.resolved.contains_key("r");
    // Two official parts overlapping in the remainder (31.3–31.7°): neither region's by id order.
    let over = run_with(&[(
        "p/_remainder.toml",
        "of = \"box:30.0,20.0,33.0,21.0\"\n[by]\nl = \"box:30.0,19.0,31.7,22.0\"\nr = \"box:31.3,19.0,33.0,22.0\"\n",
    )]);
    let p = said(&over, "overlap by");
    assert_eq!(p.len(), 1, "{:#?}", over.problems);
    assert!(p[0].found == ["l", "r"] && p[0].about == ["p"] && p[0].files == ["p/_remainder.toml"], "{:?}", p[0]);
    assert!(folder_held(&over));
    // Two overlapping only off the coast (21°N): nothing there to take, nothing said.
    let off = run_with(&[(
        "p/_remainder.toml",
        "of = \"box:30.0,20.0,33.0,21.5\"\n[by]\nl = \"box:30.0,19.0,31.7,22.0\"\nr = \"box:31.3,21.1,33.0,22.0\"\n",
    )]);
    assert!(said(&off, "overlap by").is_empty() && !folder_held(&off), "{:#?}", off.problems);
    // A region of the folder without water.
    let m = "id = \"m\"\nname = \"M\"\noutline = [\"box:40.0,40.0,40.1,40.1\"]\n";
    let dry = run_with(&[("p/m.toml", m)]);
    assert_eq!(said(&dry, "m has no water key").len(), 1, "{:#?}", dry.problems);
    assert!(folder_held(&dry));
    // A held region of the folder.
    let broken = "id = \"r\"\nname = \"R\"\noutline = [\"box:32.0,19.9,33.1,21.1\", \"land:nothing\"]\nwater = []\n";
    let held = run_with(&[("p/r.toml", broken)]);
    assert_eq!(said(&held, "r is held").len(), 1, "{:#?}", held.problems);
    assert!(!held.resolved.contains_key("l"));
    // A held region outside the folder, its outline in the remainder's cells; one far from it.
    let near = "id = \"q\"\nname = \"Q\"\noutline = [{ minus = [\"box:31.4,20.4,31.6,20.6\", \"land:nothing\"] }]\nwater = []\n";
    let out = run_with(&[("q.toml", near)]);
    let p = said(&out, "q is held, and its outline may reach");
    assert!(p.len() == 1 && p[0].found == ["q"], "{:#?}", out.problems);
    assert!(folder_held(&out));
    let far = "id = \"q\"\nname = \"Q\"\noutline = [{ minus = [\"box:60.0,60.0,60.1,60.1\", \"land:nothing\"] }]\nwater = []\n";
    let out = run_with(&[("q.toml", far)]);
    assert!(said(&out, "its outline may reach").is_empty(), "{:#?}", out.problems);
    assert!(out.resolved.contains_key("l") && out.resolved.contains_key("r"), "{:#?}", out.problems);
    // A folder whose regions have no land anywhere: its remainder's rest would go to no one.
    let mut files = all();
    files.extend([
        ("s.toml", s),
        ("n/m.toml", "id = \"m\"\nname = \"M\"\noutline = [\"box:40.0,40.0,40.1,40.1\"]\nwater = []\n"),
        ("n/_remainder.toml", "of = \"box:40.1,40.0,40.2,40.1\"\n"),
    ]);
    let out = run(&w, &files);
    let p = said(&out, "none of its regions has land or lakes");
    assert!(p.len() == 1 && p[0].files == ["n/_remainder.toml"], "{:#?}", out.problems);
    assert!(!out.resolved.contains_key("m"));
}

/// Two places' remainders overlapping, neither folder holding the other (#1448 review): which is
/// taken first says nothing of whose it is, so the second holds its folder, naming both files;
/// meeting on a line, nothing is said.
#[test]
fn two_places_remainders_overlapping_hold_the_second() {
    let w = world_more(1.002, vec![bx(30.0, 20.0, 33.0, 21.0)]);
    let l = "id = \"l\"\nname = \"L\"\noutline = [\"box:29.9,19.9,31.0,21.1\"]\nwater = []\n";
    let r = "id = \"r\"\nname = \"R\"\noutline = [\"box:32.0,19.9,33.1,21.1\"]\nwater = []\n";
    let s = "id = \"s\"\nname = \"S\"\nwater = [{ buffer = \"box:30.0,20.0,33.0,21.0\", by = \"10 km\" }]\n";
    let run_with = |p_of: &'static str, t_of: &'static str| {
        let mut files = all();
        files.extend([
            ("p/l.toml", l),
            ("t/r.toml", r),
            ("s.toml", s),
            ("p/_remainder.toml", p_of),
            ("t/_remainder.toml", t_of),
        ]);
        run(&w, &files)
    };
    let over = run_with("of = \"box:30.0,20.0,32.0,21.0\"\n", "of = \"box:31.5,20.0,33.0,21.0\"\n");
    let p: Vec<_> = over
        .problems
        .iter()
        .filter(|p| p.check == "rg-ref" && p.message.contains("meets p/_remainder.toml's"))
        .collect();
    assert_eq!(p.len(), 1, "{:#?}", over.problems);
    assert!(p[0].files == ["t/_remainder.toml", "p/_remainder.toml"] && p[0].about == ["t", "p"], "{:?}", p[0]);
    assert!(over.resolved.contains_key("l") && !over.resolved.contains_key("r"));
    let apart = run_with("of = \"box:30.0,20.0,31.5,21.0\"\n", "of = \"box:31.5,20.0,33.0,21.0\"\n");
    assert!(apart.problems.is_empty(), "{:#?}", apart.problems);
    let at = |id: &str, x: f64| area_of(&apart.resolved[id].geometry).contains([e7(x), e7(20.5)]);
    assert!(at("l", 31.4) && at("r", 31.6) && !at("r", 31.4));
}

/// A remainder whose `of` runs out to sea (#1448 review): the sea in it is cut off with its region's
/// outline, once, the land to the coastline its region's in one piece; a region without water from
/// another folder inside it keeps what it has.
#[test]
fn a_remainder_reaching_the_sea_keeps_the_coastline() {
    let w = world_more(1.002, vec![bx(30.0, 20.0, 33.0, 21.0)]);
    let l = "id = \"l\"\nname = \"L\"\noutline = [\"box:29.9,19.9,31.0,20.5\"]\nwater = []\n";
    let r = "id = \"r\"\nname = \"R\"\noutline = [\"box:32.0,19.9,33.1,20.5\"]\nwater = []\n";
    let s = "id = \"s\"\nname = \"S\"\nwater = [{ buffer = \"box:30.0,20.0,33.0,21.0\", by = \"10 km\" }]\n";
    let d = "id = \"d\"\nname = \"D\"\noutline = [\"box:31.4,20.6,31.6,20.8\"]\n";
    let mut files = all();
    files.extend([
        ("p/l.toml", l),
        ("p/r.toml", r),
        ("s.toml", s),
        ("d.toml", d),
        ("p/_remainder.toml", "of = \"box:30.0,20.0,33.0,21.5\"\n"),
    ]);
    let out = run(&w, &files);
    assert!(out.problems.is_empty(), "{:#?}", out.problems);
    let land = |id: &str| {
        let mut g = out.resolved[id].geometry.clone();
        g.shapes.retain(|s| !s.water);
        g
    };
    let has = |id: &str, x: f64, y: f64| area_of(&land(id)).contains([e7(x), e7(y)]);
    assert!(has("l", 30.5, 20.9) && has("l", 30.5, 20.999) && !has("l", 30.5, 21.2), "to the coast, no sea");
    assert!(has("r", 32.5, 20.999) && !has("r", 32.5, 21.2));
    assert!(has("d", 31.5, 20.7) && !has("l", 31.5, 20.7) && !has("r", 31.5, 20.7), "d keeps its own");
    // (One piece each, its coast the coastline's straight edge: no slivers taken back by the coast.)
    for id in ["l", "r"] {
        let g = land(id);
        assert_eq!(g.shapes.len(), 1, "{id}: {:?}", g.shapes.len());
    }
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
    // Its sea: water parts stopping short of the coast (a notch 0.008°, about 840 m, wide at the
    // land's east edge) reach it; one 0.02° wide (2.1 km: past `COAST_REACH_M`) doesn't, nor does
    // a notch off the coast.
    let s = "id = \"s\"\nname = \"S\"\noutline = [\"osm:4\"]\n\
             water = [{ minus = [\"box:19.0,19.0,22.0,22.0\", \"box:21.0,20.4,21.008,20.5\", \"box:21.0,20.6,21.02,20.7\", \"box:21.5,20.4,21.508,20.5\"] }]\n";
    let out = run(&w, &[PINS, ("s.toml", s)]);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let mut g = out.resolved["s"].geometry.clone();
    g.shapes.retain(|s| s.water);
    let water = area_of(&g);
    assert!(water.contains([e7(21.004), e7(20.45)]), "a notch within reach");
    assert!(!water.contains([e7(21.01), e7(20.65)]), "a notch past reach");
    assert!(!water.contains([e7(21.504), e7(20.45)]), "a notch off the coast");
}

/// One coastline where two regions meet on it: the land past both outlines' coasts is split at the
/// median line between them, each taking what lies nearer it.
#[test]
fn coastal_land_two_regions_border_is_split_between_them() {
    // Land to 21.0°N; two outlines side by side stopping 0.005° (about 520 m) short of it.
    let w = world_more(1.002, vec![bx(30.0, 20.0, 32.0, 21.0)]);
    let l = "id = \"l\"\nname = \"L\"\noutline = [\"box:30.0,20.0,31.0,20.995\"]\nwater = []\n";
    let r = "id = \"r\"\nname = \"R\"\noutline = [\"box:31.0,20.0,32.0,20.995\"]\nwater = []\n";
    let out = run(&w, &[PINS, ("l.toml", l), ("r.toml", r)]);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let at = |id: &str, x: f64| area_of(&out.resolved[id].geometry).contains([e7(x), e7(20.998)]);
    assert!(at("l", 30.5) && !at("r", 30.5));
    assert!(at("r", 31.5) && !at("l", 31.5));
    assert!(at("l", 30.99) && at("r", 31.01), "split near the border");
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

/// A hole in a zone's water off any coast (the zones' source has a rock OSM hasn't) is the open
/// sea's, and said: a zone's water the rest has, naming the region round it (task #1427: never
/// given to it).
#[test]
fn a_small_hole_in_the_sea_off_any_coast_is_an_error() {
    let w = world();
    let holed = "id = \"disputed\"\nname = \"Disputed zone\"\n\
                 water = [{ minus = [\"eez:2\", \"box:5.5,5.5,5.52,5.52\"] }]\n";
    let mut files = all();
    for f in files.iter_mut().filter(|f| f.0 == "disputed.toml") {
        f.1 = holed;
    }
    let out = run(&w, &files);
    assert_eq!(out.problems.len(), 1, "{:?}", out.problems);
    let p = &out.problems[0];
    assert!(p.found == vec!["rest-in-zone"] && p.message.contains("around it disputed ("), "{p:?}");
    let at = |id: &str| area_of(&out.resolved[id].geometry).contains([e7(5.51), e7(5.51)]);
    assert!(!at("disputed") && at("high-seas"));
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
    let w = World { o: buildstore::outlines::Outlines::open(&p).unwrap(), data, _d: d };
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

/// Task #1409: a divided place's own sea, kept whole as its waters region. P's zone in five
/// stretches: round Q (divided: q1, and q2 in a folder of its own under it), round p3 alone, round
/// q3 and p4, round r1 and r2, round r3 alone (R divided, with no waters region).
fn divided_world() -> World {
    let d = tempfile::tempdir().unwrap();
    let lands = [
        (51, (0.0, 1.0)),
        (52, (1.0, 2.0)),
        (53, (10.0, 11.0)),
        (54, (20.0, 21.0)),
        (55, (21.0, 22.0)),
        (56, (30.0, 31.0)),
        (57, (31.0, 32.0)),
        (58, (40.0, 41.0)),
    ];
    let p = outlines(
        d.path(),
        &lands.iter().map(|(rel, (w, e))| (*rel, "PP", deg_sq(*w, 0.0, *e, 1.0))).collect::<Vec<_>>(),
    );
    let land = lands.iter().flat_map(|(_, (w, e))| bx(*w, 0.0, *e, 1.0)).map(|r| vec![r]).collect();
    let zone = [(-1.0, 3.0), (9.0, 12.0), (19.0, 23.0), (29.0, 33.0), (39.0, 42.0)]
        .iter()
        .flat_map(|(w, e)| bx(*w, -1.0, *e, 2.0))
        .collect();
    let data = Datasets::from_parts(
        HashMap::from([(9, zone)]),
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        land,
        "fixture",
    );
    World { o: buildstore::outlines::Outlines::open(&p).unwrap(), data, _d: d }
}

/// The folder for `divided_world`, Q's waters regions left out unless `waters`.
fn divided_files(waters: bool) -> Vec<(String, String)> {
    let m = |id: &str, rel: u64| format!("id = \"{id}\"\nname = \"{id}\"\noutline = [\"osm:{rel}\"]\nwater = []\n");
    let mut v: Vec<(String, String)> = vec![
        ("_pins.toml".into(), "osm = \"2026-09-28\"\n".into()),
        (
            "p/p-waters.toml".into(),
            "id = \"p-waters\"\nname = \"P waters\"\nwater = [{ minus = [\"eez:9\", \"group:p\"] }]\nhand_out = \"p\"\n"
                .into(),
        ),
        ("p/q/q1.toml".into(), m("q1", 51)),
        ("p/q/n/q2.toml".into(), m("q2", 52)),
        ("p/p3.toml".into(), m("p3", 53)),
        ("p/q/q3.toml".into(), m("q3", 54)),
        ("p/p4.toml".into(), m("p4", 55)),
        ("p/r/r1.toml".into(), m("r1", 56)),
        ("p/r/r2.toml".into(), m("r2", 57)),
        ("p/r/r3.toml".into(), m("r3", 58)),
    ];
    if waters {
        // (Q's waters region with a little water of its own; one in Q's subfolder too, Q's business.)
        v.push((
            "p/q/q-waters.toml".into(),
            "id = \"q-waters\"\nname = \"Q waters\"\nwater = [\"box:-0.9,-0.9,-0.8,-0.8\"]\n".into(),
        ));
        v.push(("p/q/n/n-waters.toml".into(), "id = \"n-waters\"\nname = \"N waters\"\nwater = []\n".into()));
    }
    v
}

fn run_owned(w: &World, files: &[(String, String)]) -> resolve::Outcome {
    run(w, &files.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect::<Vec<_>>())
}

/// A hand-out to a country with a divided province: the stretch only the province's regions border
/// goes whole to its waters region (with its own water); one shared with another province stays the
/// country's; the undivided province takes its own; a folder without a waters region is handed to
/// region by region, as without Q's; Q's subfolder's waters region, handed nothing, ends empty
/// unreported.
#[test]
fn a_divided_places_sea_goes_whole_to_its_waters_region() {
    let w = divided_world();
    let out = run_owned(&w, &divided_files(true));
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let at = |id: &str, x: f64, y: f64| area_of(&out.resolved[id].geometry).contains([e7(x), e7(y)]);
    // Q's stretch, both its regions bordering it and across the cells' edges at 0° and 2°: whole.
    for (x, y) in [(-0.5, 0.5), (2.5, 0.5), (1.5, 1.5), (0.5, -0.5), (-0.85, -0.85)] {
        assert!(at("q-waters", x, y) && !at("p-waters", x, y), "{x}, {y}");
    }
    assert!(!at("q1", -0.5, 0.5) && !at("q2", 2.5, 0.5) && at("q1", 0.5, 0.5) && at("q2", 1.5, 0.5));
    // Q's shared with p4: P's.
    assert!(at("p-waters", 19.5, 0.5) && at("p-waters", 22.5, 0.5) && !at("q-waters", 19.5, 0.5));
    assert!(at("p3", 9.5, 0.5) && at("p3", 11.5, 0.5) && !at("p-waters", 9.5, 0.5));
    assert!(at("p-waters", 29.5, 0.5) && at("r3", 39.5, 0.5));
    assert!(!out.resolved.contains_key("n-waters"));
    // Without Q's waters regions: Q's stretch is P's (q1 and q2 both border it), and every region
    // but Q's waters as with them.
    let before = run_owned(&w, &divided_files(false));
    assert!(before.problems.is_empty(), "{:?}", before.problems);
    assert!(area_of(&before.resolved["p-waters"].geometry).contains([e7(-0.5), e7(0.5)]));
    for id in ["q1", "q2", "q3", "p3", "p4", "r1", "r2", "r3"] {
        assert_eq!(before.resolved[id], out.resolved[id], "{id}");
    }
}

/// A `_group.toml` changes no resolution, nor any key: no resolution reads one.
#[test]
fn a_group_file_changes_nothing() {
    let w = divided_world();
    let bare = divided_files(true);
    let mut named = bare.clone();
    named.push(("p/_group.toml".into(), "name = \"P\"\norder = 2\n".into()));
    named.push(("p/q/_group.toml".into(), "name = \"Q\"\n".into()));
    let (a, b) = (run_owned(&w, &bare), run_owned(&w, &named));
    assert!(a.problems.is_empty(), "{:?}", a.problems);
    assert_eq!((&a.resolved, &a.keys, &a.problems), (&b.resolved, &b.keys, &b.problems));
}

/// A waters region with `water = []` in a folder no hand-out gives to has no area: `rg-empty`.
#[test]
fn a_waters_region_no_hand_out_reaches_is_empty() {
    let w = world();
    let lone = "id = \"lone\"\nname = \"Lone waters\"\nwater = []\n";
    let out = run(&w, &[PINS, ("lone.toml", lone)]);
    assert!(
        out.problems.iter().any(|p| p.check == "rg-empty" && p.about.contains(&"lone".to_string())),
        "{:?}",
        out.problems
    );
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
    let w = World { o: buildstore::outlines::Outlines::open(&p).unwrap(), data, _d: d };
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

/// With `cover = "earth"`: a region's piece of water touching none of its land, lying across a 2°
/// cells' edge, is said once, whole, at a point inside it, naming the region beside it; it stays
/// the region's (task #1427). A region of water alone has no land to touch.
#[test]
fn water_touching_none_of_its_land_is_an_error() {
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
    let w = World { o: buildstore::outlines::Outlines::open(&p).unwrap(), data, _d: d };
    // s: land far away, and a piece of 4.4 × 5.5 km across the edge at 2°; t: water above it.
    let s = "id = \"s\"\nname = \"S\"\noutline = [\"osm:4\"]\nwater = [\"box:1.98,0.0,2.02,0.05\", { buffer = \"land:s\", by = \"10 km\" }]\n";
    let t = "id = \"t\"\nname = \"T\"\nwater = [\"box:1.9,0.05,2.1,0.2\"]\n";
    let out = run(&w, &[PINS, RULES, ("s.toml", s), ("t.toml", t), ("high-seas.toml", HIGH)]);
    assert_eq!(out.problems.len(), 1, "{:?}", out.problems);
    let p = &out.problems[0];
    assert!(p.found[0] == "stray" && p.about == vec!["s"] && p.message.starts_with("s: 24."), "{p:?}");
    assert!(p.message.contains("around it t ("), "{}", p.message);
    let at = at_of(&p.message);
    assert!((1.98..2.02).contains(&at[0]) && (0.0..0.05).contains(&at[1]), "{}", p.message);
    let at = |id: &str, x: f64, y: f64| area_of(&out.resolved[id].geometry).contains([e7(x), e7(y)]);
    assert!(at("s", 1.99, 0.02) && at("s", 2.01, 0.02) && !at("t", 1.99, 0.02), "nothing moved");
}

/// With `cover = "earth"`: a big lake in a region's outline or named by its water is its, wherever
/// its shores are: one whose shore is all another region's land is no stray water (task #1427).
#[test]
fn a_lake_whose_shores_are_anothers_is_no_stray() {
    let d = tempfile::tempdir().unwrap();
    let p = outlines(d.path(), &[(4, "DD", deg_sq(20.0, 20.0, 21.0, 21.0))]);
    let lakes = HashMap::from([(77, bx(10.4, 10.4, 10.6, 10.6)), (78, bx(10.7, 10.4, 10.9, 10.6))]);
    let land = vec![bx(10.0, 10.0, 11.0, 11.0), bx(12.0, 12.0, 12.1, 12.1)];
    let data = Datasets::from_parts(HashMap::new(), HashMap::new(), HashMap::new(), lakes, land, "fixture");
    let w = World { o: buildstore::outlines::Outlines::open(&p).unwrap(), data, _d: d };
    // u: one lake in its outline, the other in its water, and a square of land far away; v: the
    // land round the lakes, and its sea.
    let u = "id = \"u\"\nname = \"U\"\noutline = [\"box:10.4,10.4,10.6,10.6\", \"box:11.95,11.95,12.15,12.15\"]\nwater = [{ buffer = \"land:u\", by = \"10 km\" }, \"box:10.7,10.4,10.9,10.6\"]\n";
    let v = "id = \"v\"\nname = \"V\"\noutline = [{ minus = [\"box:9.95,9.95,11.05,11.05\", \"box:10.4,10.4,10.6,10.6\", \"box:10.7,10.4,10.9,10.6\"] }]\nwater = [{ minus = [{ buffer = \"land:v\", by = \"10 km\" }, \"box:10.4,10.4,10.6,10.6\", \"box:10.7,10.4,10.9,10.6\"] }]\n";
    let out = run(&w, &[PINS, RULES, ("u.toml", u), ("v.toml", v), ("high-seas.toml", HIGH)]);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let at = |id: &str, x: f64, y: f64| area_of(&out.resolved[id].geometry).contains([e7(x), e7(y)]);
    assert!(at("u", 10.5, 10.5) && !at("v", 10.5, 10.5), "the outline's lake is u's");
    assert!(at("u", 10.8, 10.5) && !at("v", 10.8, 10.5), "the water's lake is u's");
}

/// With `cover = "earth"`: a stretch of a region's water a centimetre from one touching its land
/// (the operations' snapping parts pieces meeting exactly) is one with it, no stray; one 2 km off
/// is (task #1427).
#[test]
fn a_stretch_a_centimetre_off_its_water_is_no_stray() {
    let d = tempfile::tempdir().unwrap();
    let p = outlines(d.path(), &[(4, "DD", deg_sq(20.0, 20.0, 21.0, 21.0))]);
    let land = vec![bx(10.0, 10.0, 11.0, 11.0)];
    let data = Datasets::from_parts(HashMap::new(), HashMap::new(), HashMap::new(), HashMap::new(), land, "fixture");
    let w = World { o: buildstore::outlines::Outlines::open(&p).unwrap(), data, _d: d };
    let near = "id = \"n\"\nname = \"N\"\noutline = [\"box:9.9,9.9,11.0,11.1\"]\nwater = [\"box:11.0,10.0,11.5,11.0\", \"box:11.5000001,10.2,11.6,10.8\"]\n";
    let out = run(&w, &[PINS, RULES, ("n.toml", near), ("high-seas.toml", HIGH)]);
    let strays: Vec<_> = out.problems.iter().filter(|p| p.found.first().is_some_and(|f| f == "stray")).collect();
    assert!(strays.is_empty(), "{:?}", out.problems);
    let far = "id = \"n\"\nname = \"N\"\noutline = [\"box:9.9,9.9,11.0,11.1\"]\nwater = [\"box:11.0,10.0,11.5,11.0\", \"box:11.52,10.2,11.6,10.8\"]\n";
    let out = run(&w, &[PINS, RULES, ("n.toml", far), ("high-seas.toml", HIGH)]);
    let strays: Vec<_> = out.problems.iter().filter(|p| p.found.first().is_some_and(|f| f == "stray")).collect();
    assert_eq!(strays.len(), 1, "{:?}", out.problems);
}

/// The coast: sea between one region's shore and a big lake over the sea that another's outline
/// holds (task #1427) is the shore's region's, never the lake's
/// holder's (land dominates the sea: given to the lake's holder, it touched none of its land).
#[test]
fn sea_between_a_shore_and_anothers_lake_is_the_shores() {
    let d = tempfile::tempdir().unwrap();
    let p = outlines(d.path(), &[(4, "DD", deg_sq(20.0, 20.0, 21.0, 21.0))]);
    let lakes = HashMap::from([(77, bx(11.003, 10.4, 11.2, 10.6))]);
    let land = vec![bx(10.0, 10.0, 11.0, 11.0), bx(11.2, 10.4, 11.3, 10.6)];
    let data = Datasets::from_parts(HashMap::new(), HashMap::new(), HashMap::new(), lakes, land, "fixture");
    let w = World { o: buildstore::outlines::Outlines::open(&p).unwrap(), data, _d: d };
    // k: the land to 11.0° and its sea but the box east of it; c: the lake (from 11.003°, 330 m
    // off k's shore) and the land past it, and its sea round that land. The strip between is no one's.
    let k = "id = \"k\"\nname = \"K\"\noutline = [\"box:9.9,9.9,11.0,11.1\"]\nwater = [{ minus = [{ buffer = \"land:k\", by = \"10 km\" }, \"box:11.0,10.4,11.3,10.6\"] }]\n";
    let c = "id = \"c\"\nname = \"C\"\noutline = [\"box:11.0,10.4,11.3,10.6\"]\nwater = [{ buffer = \"land:c\", by = \"10 km\" }]\n";
    let out = run(&w, &[PINS, RULES, ("k.toml", k), ("c.toml", c), ("high-seas.toml", HIGH)]);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let at = |id: &str, x: f64, y: f64| area_of(&out.resolved[id].geometry).contains([e7(x), e7(y)]);
    assert!(at("k", 11.0015, 10.5) && !at("c", 11.0015, 10.5), "the strip is k's");
    assert!(at("c", 11.1, 10.5), "the lake is c's");
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

/// Land two regions with water both have is an overlap, at a point inside it, naming both; each
/// keeps it, however small (task #1427).
#[test]
fn land_in_two_regions_with_water_is_an_overlap() {
    let w = world();
    for (b, k) in [("0.8", "123."), ("0.72", "4.9")] {
        let dup = format!("id = \"dup\"\nname = \"Dup\"\noutline = [\"box:0.7,0.7,{b},{b}\"]\nwater = []\n");
        let mut files = all();
        files.push(("w/dup.toml", &dup));
        let out = run(&w, &files);
        assert_eq!(out.problems.len(), 1, "{:?}", out.problems);
        let p = &out.problems[0];
        assert!(!p.warning && p.check == "rg-cover" && p.found == vec!["overlap"], "{p:?}");
        assert_eq!(p.about, vec!["a", "dup"]);
        assert!(p.message.contains(&format!("both cover {k}")), "{}", p.message);
        let at = at_of(&p.message);
        assert!((0.7..0.8).contains(&at[0]) && (0.7..0.8).contains(&at[1]), "{}", p.message);
        for id in ["a", "dup"] {
            assert!(area_of(&out.resolved[id].geometry).contains([e7(0.71), e7(0.71)]), "{id} keeps it");
        }
    }
}

/// A nearest-coast split decides by the nearest coast however far it is: coasts 670 and 780 km off
/// (past the first reach of 450 km) still split the sea.
#[test]
fn a_nearest_coast_split_reaches_past_450_km() {
    let within = Area::from_rings(&bx(0.0, 0.0, 0.1, 0.1));
    let east = Area::from_rings(&bx(6.1, 0.0, 6.2, 0.1));
    let west = Area::from_rings(&bx(-7.1, 0.0, -7.0, 0.1));
    assert!(geom::nearest_within(&within, &[west.clone()], &[east.clone()], 1000.0, 450_000.0).is_none());
    assert!(geom::nearest_growing(&within, &[west.clone()], &[east.clone()], 1000.0).is_empty());
    assert_eq!(geom::nearest_growing(&within, &[east], &[west], 1000.0), within);
}

// ---- The coastline's land (crate::inputs::regions::land) ----

fn lp(x: f64, y: f64) -> geom::Pt {
    [(x * 1e7) as i64, (y * 1e7) as i64]
}

/// A way of nodes `ids` at `pts`.
fn way(ids: &[i64], pts: &[geom::Pt]) -> Vec<(i64, geom::Pt)> {
    ids.iter().copied().zip(pts.iter().copied()).collect()
}

#[test]
fn coastline_ways_join_into_rings_gaps_close_and_the_pole_is_closed_along() {
    let mut stats = land::Stats::default();
    // An island in two ways (land on their left: counter-clockwise).
    let a = way(&[1, 2, 3], &[lp(0.0, 0.0), lp(1.0, 0.0), lp(1.0, 1.0)]);
    let b = way(&[3, 4, 1], &[lp(1.0, 1.0), lp(0.0, 1.0), lp(0.0, 0.0)]);
    // Another, its ring broken at two gaps of about 110 m: closed.
    let c = way(&[10, 11, 12, 13], &[lp(5.0, 0.0), lp(6.0, 0.0), lp(6.0, 1.0), lp(5.0, 1.0)]);
    let d = way(&[14, 15], &[lp(5.0, 0.999), lp(5.0, 0.001)]);
    // A chain ending 50 km from anything: left out.
    let e = way(&[20, 21, 22], &[lp(10.0, 0.0), lp(11.0, 0.0), lp(11.0, 1.0)]);
    // A coast round the south pole, running west across ±180°.
    let f = way(&[30, 31, 32, 33], &[lp(170.0, -70.0), lp(60.0, -68.0), lp(-60.0, -72.0), lp(-170.0, -70.0)]);
    let g = way(&[33, 30], &[lp(-170.0, -70.0), lp(170.0, -70.0)]);
    let rings = land::assemble(vec![a, b, c, d, e, f, g], &mut stats);
    assert_eq!((stats.rings, stats.gaps_closed, stats.chains_dropped, stats.polar), (3, 2, 1, 1), "{stats:?}");
    let t = tiles::TArea::from_rings(&rings);
    let at = |x: f64, y: f64| {
        let q = lp(x, y);
        t.get((q[0].div_euclid(tiles::CELL), q[1].div_euclid(tiles::CELL))).contains(q)
    };
    assert!(at(0.5, 0.5) && at(5.5, 0.5) && !at(10.5, 0.5) && !at(2.0, 0.5));
    assert!(at(0.0, -80.0) && at(179.5, -75.0) && at(-179.5, -75.0) && !at(0.0, -60.0), "Antarctica");
}

#[test]
fn a_coastline_way_drawn_the_wrong_way_is_turned_round() {
    let mut stats = land::Stats::default();
    // An island in three ways, the middle one (2 → 3) drawn 3 → 2: one ring, no gap bridged.
    let a = way(&[1, 2], &[lp(0.0, 0.0), lp(1.0, 0.0)]);
    let b = way(&[3, 2], &[lp(1.0, 1.0), lp(1.0, 0.0)]);
    let c = way(&[3, 4, 1], &[lp(1.0, 1.0), lp(0.0, 1.0), lp(0.0, 0.0)]);
    let rings = land::assemble(vec![a, b, c], &mut stats);
    assert_eq!((stats.rings, stats.reversed, stats.gaps_closed, stats.chains_dropped), (1, 1, 0, 0), "{stats:?}");
    assert_eq!(rings[0].len(), 4);
}

#[test]
fn the_land_file_reads_back_by_cell() {
    let rings = vec![vec![lp(0.5, 0.5), lp(3.5, 0.5), lp(3.5, 1.5), lp(0.5, 1.5)]];
    let t = tiles::TArea::from_rings(&rings);
    let b = land::encode("2026-09-28", &land::Stats::default(), &t);
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("l.bin"), &b).unwrap();
    let l = land::Land::open(std::fs::File::open(d.path().join("l.bin")).unwrap()).unwrap();
    for (c, a) in &t.cells {
        assert_eq!(*l.cell(*c), *a);
    }
    assert!(l.cell((40, 40)).is_empty() && !l.is_empty());
    assert!(l.failed().is_ok());
    // A file cut short: its last cell reads as no land and fails the land, no panic.
    std::fs::write(d.path().join("s.bin"), &b[..b.len() - 3]).unwrap();
    let s = land::Land::open(std::fs::File::open(d.path().join("s.bin")).unwrap()).unwrap();
    let last = *t.cells.keys().last().unwrap();
    assert!(s.cell(last).is_empty());
    assert!(s.failed().unwrap_err().to_string().contains("the regions' land"));
}

/// Sea no region has by a coast (a lagoon, a creek the zones stop short of) or near one (a hole in
/// a zone's water 20 km off a coast) is a gap at a point inside it, naming the regions round it,
/// never the rest's nor a neighbour's (task #1427); the open sea is the rest's.
#[test]
fn sea_the_zones_stop_short_of_is_an_error() {
    let ring = |w: f64, s: f64, e: f64, n: f64| bx(w, s, e, n).remove(0);
    // One land with a lagoon (about 1,000 km²) inside one region, one with a creek across two.
    let lagoon = vec![ring(30.0, 30.0, 31.0, 31.0), ring(30.3, 30.3, 30.7, 30.6)];
    let creek = vec![ring(40.0, 40.0, 42.0, 41.0), ring(40.8, 40.2, 41.2, 40.8)];
    // Its zone round it with a hole (about 1,900 km²) 20 km off its coast, touching no land; the
    // creek's land's zone round it, stopping at its coast.
    let zone = vec![ring(29.5, 29.5, 32.0, 31.5), ring(30.0, 30.0, 31.0, 31.0), ring(31.2, 30.2, 31.6, 30.6)];
    let zone7 = vec![ring(39.5, 39.5, 42.5, 41.5), ring(40.0, 40.0, 42.0, 41.0)];
    let w = world_zones(1.002, vec![lagoon, creek], HashMap::from([(4, zone), (7, zone7)]));
    let lag = "id = \"lag\"\nname = \"Lagoon land\"\noutline = [\"box:29.9,29.9,31.1,31.1\"]\nwater = [\"eez:4\"]\n";
    let west = "id = \"west\"\nname = \"West\"\noutline = [\"box:39.9,39.9,41.0,41.1\"]\nwater = []\n";
    let east = "id = \"east\"\nname = \"East\"\noutline = [\"box:41.0,39.9,42.1,41.1\"]\nwater = []\n";
    let cw = "id = \"cw\"\nname = \"Creek waters\"\nwater = [\"eez:7\"]\n";
    let mut files = all();
    files.extend([("lag.toml", lag), ("west.toml", west), ("east.toml", east), ("cw.toml", cw)]);
    let out = run(&w, &files);
    let at = |id: &str, x: f64, y: f64| area_of(&out.resolved[id].geometry).contains([e7(x), e7(y)]);
    let owner =
        |x: f64, y: f64| -> Vec<&str> { out.resolved.keys().filter(|id| at(id, x, y)).map(String::as_str).collect() };
    for (x, y) in [(30.5, 30.45), (31.4, 30.4), (40.9, 40.5), (41.1, 40.5)] {
        assert!(owner(x, y).is_empty(), "{x}, {y}: {:?}", owner(x, y));
    }
    assert_eq!(owner(50.0, 50.0), vec!["high-seas"]);
    let mut said: Vec<(Vec<f64>, &str)> = out
        .problems
        .iter()
        .map(|p| {
            assert!(!p.warning && p.check == "rg-cover" && p.found == vec!["gap"], "{p:?}");
            (at_of(&p.message), p.message.as_str())
        })
        .collect();
    said.sort_by(|a, b| a.0[0].total_cmp(&b.0[0]));
    assert_eq!(said.len(), 3, "{said:?}");
    assert!((30.3..30.7).contains(&said[0].0[0]) && said[0].1.contains("around it lag ("), "the lagoon: {said:?}");
    assert!((31.2..31.6).contains(&said[1].0[0]) && said[1].1.contains("around it lag ("), "the hole: {said:?}");
    assert!((40.8..41.2).contains(&said[2].0[0]), "the creek: {said:?}");
    assert!(said[2].1.contains("east (") && said[2].1.contains("west ("), "{said:?}");
}

/// A zone no region names and none borders is the rest's, and an error: the high seas take only
/// the water beyond every zone (#1127).
#[test]
fn a_zone_no_region_has_is_an_error() {
    let w = world_zones(1.002, Vec::new(), HashMap::from([(3, bx(60.0, 60.0, 60.5, 60.5))]));
    let out = run(&w, &all());
    assert!(
        out.problems.iter().any(|p| !p.warning && p.check == "rg-cover" && p.found == vec!["rest-in-zone"]),
        "{:?}",
        out.problems
    );
    assert!(out.fit.gaps.iter().any(|g| g.rest && g.zone_km2 > 1000.0), "{:?}", out.fit.gaps);
}

/// The open sea stays the rest's: past a coast's own water, and in a hole in the zones' water 100
/// km or more from any land (the Gulf of Mexico's, the Sea of Okhotsk's high seas) though land lies
/// in the cells round it (#1127). Sea touching a coast is a country's: with no region's water
/// there, a gap (task #1427).
#[test]
fn the_open_sea_by_a_coast_stays_the_high_seas() {
    let ring = |w: f64, s: f64, e: f64, n: f64| bx(w, s, e, n).remove(0);
    // A land no zone has; an island in a zone with a hole (about 12,000 km²) 185 km from it.
    let bare = vec![ring(60.0, 0.2, 61.0, 0.8)];
    let island = vec![ring(45.5, 50.2, 45.7, 50.4)];
    let zone = vec![ring(40.0, 49.0, 46.5, 54.0), ring(42.5, 51.5, 43.5, 52.5)];
    let w = world_zones(1.002, vec![bare, island], HashMap::from([(5, zone)]));
    // (The hole's cells have land round them: not the open sea by that test, by its distance.)
    assert!(!tiles::far_from_land(&w.data, (21, 25)) && !tiles::far_from_land(&w.data, (21, 26)));
    let isl = "id = \"dough\"\nname = \"Dough\"\noutline = [\"box:45.4,50.1,45.8,50.5\"]\nwater = [\"eez:5\"]\n";
    let bare = "id = \"bare\"\nname = \"Bare\"\noutline = [\"box:59.9,0.1,61.1,0.9\"]\n\
                water = [{ buffer = \"land:bare\", by = \"30 km\" }]\n";
    let mut files = all();
    files.extend([("bare.toml", bare), ("dough.toml", isl)]);
    let out = run(&w, &files);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let at = |id: &str, x: f64, y: f64| area_of(&out.resolved[id].geometry).contains([e7(x), e7(y)]);
    let owner =
        |x: f64, y: f64| -> Vec<&str> { out.resolved.keys().filter(|id| at(id, x, y)).map(String::as_str).collect() };
    assert_eq!(owner(61.5, 0.5), vec!["high-seas"], "55 km off its coast, past its water");
    assert_eq!(owner(61.1, 0.5), vec!["bare"]);
    assert_eq!(owner(43.0, 52.0), vec!["high-seas"], "the hole in the zone");
    assert_eq!(owner(44.0, 52.0), vec!["dough"], "its zone");
    // Its coast with no water: the sea touching it is a gap, said by the coast.
    let bare = "id = \"bare\"\nname = \"Bare\"\noutline = [\"box:59.9,0.1,61.1,0.9\"]\nwater = []\n";
    let mut files = all();
    files.extend([("bare.toml", bare), ("dough.toml", isl)]);
    let out = run(&w, &files);
    let touching = |g: &&tiles::GapFit| !g.rest && g.touches && g.edges.iter().any(|(id, _)| id == "bare");
    assert!(out.fit.gaps.iter().any(|g| touching(&g)), "{:?}", out.problems);
}

/// The open sea's test reaches at least 110 km either side whatever the latitude: a column of cells
/// narrows toward a pole, so more of them are read there (#1127).
#[test]
fn the_open_sea_is_far_from_land_at_any_latitude() {
    let land = vec![bx(0.5, 0.5, 1.0, 1.0), bx(0.5, 80.5, 1.0, 81.0)];
    let data = Datasets::from_parts(HashMap::new(), HashMap::new(), HashMap::new(), HashMap::new(), land, "fixture");
    assert!(!tiles::far_from_land(&data, (1, 0)));
    assert!(tiles::far_from_land(&data, (2, 0)), "a column on, 2° at the equator");
    assert!(!tiles::far_from_land(&data, (2, 40)), "two columns on at 82°: 60 km");
    assert!(!tiles::far_from_land(&data, (4, 40)));
    assert!(tiles::far_from_land(&data, (5, 40)), "five columns on at 82°: 150 km");
    assert!(!tiles::far_from_land(&data, (3, 41)), "three columns on at 84°: 70 km");
    assert!(tiles::far_from_land(&data, (0, 42)), "two rows on");
}

/// Task #1426: an area cut down round a box (`tiles::near_part`) meets anything inside the box as
/// the whole does, on a thread that snaps as the hand-outs do (`geom::snapping`): a wiggly coast
/// with a hole and a far island, and a comb whose teeth cross the cut's edge again and again,
/// against a stretch's ring of 100 m; a long segment crossing both the box and the cut is refused
/// (None); nothing near, empty.
#[test]
fn an_area_cut_down_round_a_box_meets_what_is_inside_it_the_same() {
    use super::tiles::near_part;
    use det::Det;
    geom::snapping(|| {
        let coast: geom::Ring = (0..=2000)
            .map(|i| {
                let t = i as f64 / 2000.0;
                [e7(t * 2.0), e7(1.0 + 0.003 * (t * 400.0).dsin() + 0.001 * (t * 1300.0).dcos())]
            })
            .chain([[e7(2.0), e7(0.0)], [e7(0.0), e7(0.0)]])
            .collect();
        let hole = bx(1.0, 0.5, 1.2, 0.7).remove(0);
        let island = bx(1.9, 1.8, 1.95, 1.85);
        let q = Area::from_rings(&[vec![coast, hole], island].concat());
        // A comb from 3°E: teeth 0.004° wide and 0.03° tall every 0.008°, drawn with a point every
        // 0.001° (zigzagging by 1 m, so none is dropped as collinear), so each crosses the cut (about
        // 0.01° out) on short segments.
        let mut comb: geom::Ring = vec![[e7(3.0), e7(0.0)], [e7(3.2), e7(0.0)]];
        for k in (0..25).rev() {
            let x = 3.0 + 0.008 * k as f64;
            for j in 0..=30 {
                comb.push([e7(x + 0.004 + 0.00001 * (j % 2) as f64), e7(1.0 + 0.001 * j as f64)]);
            }
            for j in (0..=30).rev() {
                comb.push([e7(x + 0.00001 * (j % 2) as f64), e7(1.0 + 0.001 * j as f64)]);
            }
        }
        let comb = Area::from_rings(&[comb]);
        for (a, w, s) in [
            (&q, 0.3, 0.99),
            (&q, 1.1, 0.4),
            (&q, 1.0, 0.45),
            (&q, 1.7, 0.98),
            (&q, 1.915, 1.815),
            (&comb, 3.09, 1.012),
            (&comb, 3.1, 0.997),
        ] {
            let inner = Area::from_rings(&bx(w, s, w + 0.004, s + 0.004));
            let ring = geom::buffer_area_coarse(&inner, 100.0).minus(&inner);
            let rb = ring.bbox().unwrap();
            let cut = near_part(a, rb).unwrap_or_else(|| panic!("{w}, {s}: no long segment near"));
            assert!(geom::points(&cut) < geom::points(a), "{w}, {s}: cut down");
            assert!(!ring.intersect(a).is_empty(), "{w}, {s}: the ring meets it");
            assert_eq!(ring.intersect(&cut), ring.intersect(a), "{w}, {s}");
        }
        // A box with nothing near: empty.
        assert!(near_part(&q, [e7(5.0), e7(5.0), e7(5.1), e7(5.1)]).is_some_and(|a| a.is_empty()));
        // The coast's long closing edge along 0°E, and a triangle's long edge, crossing the box and
        // running on past the cut: read whole.
        assert!(near_part(&q, [e7(-0.001), e7(0.9), e7(0.02), e7(0.92)]).is_none());
        let tri = Area::from_rings(&[vec![[e7(0.0), e7(0.0)], [e7(2.0), e7(2.0)], [e7(0.0), e7(2.0)]]]);
        assert!(near_part(&tri, [e7(0.99), e7(0.99), e7(1.01), e7(1.01)]).is_none());
    });
}

/// Task #1426: a stretch's edge with a member (`tiles::edge_in`, reading the member's part round it)
/// is the edge read against the member's whole cells, to the bit, snapping: across ±180° (the
/// member's cell wrapped and shifted) and near 80°N.
#[test]
fn a_stretchs_edge_is_the_same_read_against_the_members_part() {
    use super::tiles::{boxes_of, cell_box, edge_in, TArea, CELL};
    use det::Det;
    // The edge as it was read: the ring against each neighbouring cell's whole piece.
    let whole = |p: &Area, c: (i64, i64), t: &TArea| -> f64 {
        let ring = geom::buffer_area_coarse(p, 100.0).minus(p);
        let mut sum = 0.0;
        for x in c.0 - 1..=c.0 + 1 {
            let w = (x + 90).rem_euclid(180) - 90;
            let shift = (x - w) * CELL;
            for y in c.1 - 1..=c.1 + 1 {
                if let Some(q) = t.cells.get(&(w, y)) {
                    sum += geom::area_km2(&ring.intersect(&q.shifted(shift))) / 0.1;
                }
            }
        }
        sum
    };
    // A member whose east side wiggles at `e`° with a point every 0.001°, from `s`° to `n`°.
    let member = |w: f64, s: f64, n: f64, e: f64| -> TArea {
        let k = ((n - s) / 0.001).round() as usize;
        let mut r: geom::Ring = vec![[e7(w), e7(n)], [e7(w), e7(s)]];
        r.extend((0..=k).map(|i| {
            let y = s + 0.001 * i as f64;
            [e7(e - 0.002 + 0.002 * (y * 900.0).dsin()), e7(y)]
        }));
        TArea::from_rings(&[r])
    };
    geom::snapping(|| {
        for (t, p) in [
            // West of ±180° (cell 89), a stretch just east of it (cell −90).
            (member(179.0, 10.2, 11.0, 180.0), bx(-180.0, 10.5, -179.9, 10.6)),
            // Near 80°N: the member in one column, the stretch in the next.
            (member(11.0, 80.3, 81.2, 12.0), bx(12.0, 80.6, 12.3, 80.7)),
        ] {
            let p = Area::from_rings(&p);
            let pb = p.bbox().unwrap();
            let c = (pb[0].div_euclid(CELL), pb[1].div_euclid(CELL));
            let p = p.intersect(&geom::box_area(cell_box(c)));
            let got = edge_in(&p, c, &t, &boxes_of(&t), &mut None);
            assert!(got > 0.0, "{c:?}: the stretch touches the member");
            assert_eq!(got.to_bits(), whole(&p, c, &t).to_bits(), "{c:?}");
        }
    });
}

/// Task #1426: two places' waters handing out, touching no region in common, are made together (one
/// run of hand-outs): each place's regions resolve as they do with the other place left out.
#[test]
fn hand_outs_made_together_give_what_each_gives_alone() {
    let d = tempfile::tempdir().unwrap();
    // Each place: two regions sharing a stretch of its zone, a third with a stretch of its own.
    let places = [("a", 0.0, 61u64), ("b", 40.0, 71)];
    let mut lands = Vec::new();
    for (_, x, rel) in places {
        lands.extend([(rel, (x, x + 1.0)), (rel + 1, (x + 1.0, x + 2.0)), (rel + 2, (x + 10.0, x + 11.0))]);
    }
    let p = outlines(
        d.path(),
        &lands.iter().map(|(rel, (w, e))| (*rel, "PP", deg_sq(*w, 0.0, *e, 1.0))).collect::<Vec<_>>(),
    );
    let land = lands.iter().flat_map(|(_, (w, e))| bx(*w, 0.0, *e, 1.0)).map(|r| vec![r]).collect();
    let zones = places
        .iter()
        .enumerate()
        .map(|(k, (_, x, _))| {
            (k as u64 + 1, [bx(x - 1.0, -1.0, x + 3.0, 2.0), bx(x + 9.0, -1.0, x + 12.0, 2.0)].concat())
        })
        .collect();
    let data = Datasets::from_parts(zones, HashMap::new(), HashMap::new(), HashMap::new(), land, "fixture");
    let w = World { o: buildstore::outlines::Outlines::open(&p).unwrap(), data, _d: d };
    let m = |id: &str, rel: u64| format!("id = \"{id}\"\nname = \"{id}\"\noutline = [\"osm:{rel}\"]\nwater = []\n");
    let files = |which: &[&str]| -> Vec<(String, String)> {
        let mut v = vec![("_pins.toml".to_string(), "osm = \"2026-09-28\"\n".to_string())];
        for (k, (g, _, rel)) in places.iter().enumerate() {
            if !which.contains(g) {
                continue;
            }
            for (i, r) in [1u64, 2, 3].iter().enumerate() {
                v.push((format!("{g}/{g}{r}.toml"), m(&format!("{g}{r}"), rel + i as u64)));
            }
            v.push((
                format!("{g}/{g}-waters.toml"),
                format!("id = \"{g}-waters\"\nname = \"{g} waters\"\nwater = [{{ minus = [\"eez:{}\", \"group:{g}\"] }}]\nhand_out = \"{g}\"\n", k + 1),
            ));
        }
        v
    };
    let both = run_owned(&w, &files(&["a", "b"]));
    assert!(both.problems.is_empty(), "{:?}", both.problems);
    for g in ["a", "b"] {
        let alone = run_owned(&w, &files(&[g]));
        assert!(alone.problems.is_empty(), "{:?}", alone.problems);
        for (id, r) in &alone.resolved {
            assert_eq!(&both.resolved[id], r, "{id}");
        }
        // (The stretch round the third region was handed to it.)
        let x = if g == "a" { 9.5 } else { 49.5 };
        assert!(area_of(&both.resolved[&format!("{g}3")].geometry).contains([e7(x), e7(0.5)]), "{g}3");
    }
}

/// A hand-out: a stretch of the handing region's water that two members border stays the handing
/// region's, however small (task #1427: the old fill gave one under `sliver_km2` to the member with
/// the longest edge).
#[test]
fn a_stretch_several_members_border_stays_the_handing_regions() {
    let d = tempfile::tempdir().unwrap();
    let p = outlines(d.path(), &[(61, "PP", deg_sq(0.0, 0.0, 1.0, 1.0)), (62, "PP", deg_sq(1.0, 0.0, 2.0, 1.0))]);
    let land = vec![bx(0.0, 0.0, 1.0, 1.0), bx(1.0, 0.0, 2.0, 1.0)];
    // (The zone: a strip off the coast, 0.07° of it off p1's and 0.03° off p2's: about 36 km².)
    let zones = HashMap::from([(1u64, bx(0.93, 1.0, 1.03, 1.03))]);
    let data = Datasets::from_parts(zones, HashMap::new(), HashMap::new(), HashMap::new(), land, "fixture");
    let w = World { o: buildstore::outlines::Outlines::open(&p).unwrap(), data, _d: d };
    let f = |p: &str, t: &str| (p.to_string(), t.to_string());
    let files = vec![
        f("_pins.toml", "osm = \"2026-09-28\"\n"),
        f("p/p1.toml", "id = \"p1\"\nname = \"p1\"\noutline = [\"osm:61\"]\nwater = []\n"),
        f("p/p2.toml", "id = \"p2\"\nname = \"p2\"\noutline = [\"osm:62\"]\nwater = []\n"),
        f(
            "p/p-waters.toml",
            "id = \"p-waters\"\nname = \"P waters\"\nwater = [{ minus = [\"eez:1\", \"group:p\"] }]\nhand_out = \"p\"\n",
        ),
    ];
    let out = run_owned(&w, &files);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let at = |id: &str| area_of(&out.resolved[id].geometry).contains([e7(0.98), e7(1.015)]);
    assert!(at("p-waters") && !at("p1") && !at("p2"));
}
