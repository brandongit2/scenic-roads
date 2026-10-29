//! The regions' check: resolution against a pass's outlines, the operations, the antimeridian, and
//! the gate holding and taking in region files (docs/inputs.md §6.1.10's list of tests).

use super::*;
use crate::agent::recipes::Recipe;
use crate::coverage::Coverage;
use crate::inputs::gate::{decide, Given};
use crate::inputs::{FileEntry, Index, Listing};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

/// An outline set made from GeoJSON polygons (degrees): (relation id, ISO 3166-1 code, rings).
pub(crate) fn outlines(dir: &Path, rels: &[(u64, &str, Vec<Vec<[f64; 2]>>)]) -> std::path::PathBuf {
    let mut text = String::new();
    for (id, iso, rings) in rels {
        let coords = serde_json::to_string(rings).unwrap();
        text += &format!("\u{1e}{{\"type\":\"Feature\",\"geometry\":{{\"type\":\"Polygon\",\"coordinates\":{coords}}},\"properties\":{{\"@type\":\"relation\",\"@id\":{id},\"boundary\":\"administrative\",\"admin_level\":\"2\",\"name\":\"R{id}\",\"ISO3166-1\":\"{iso}\"}}}}\n");
    }
    let geo = dir.join("o.geojsonseq");
    std::fs::write(&geo, text).unwrap();
    let out = dir.join("outlines.sect");
    crate::outlines::assemble_geojsonseq(&geo, &out).unwrap();
    out
}

fn sq(x: f64, y: f64, s: f64) -> Vec<Vec<[f64; 2]>> {
    vec![vec![[x, y], [x + s, y], [x + s, y + s], [x, y + s], [x, y]]]
}

const PASS: &str = "2026-09-28";

struct Fx {
    _d: tempfile::TempDir,
    o: Outlines,
}

/// Countries: 1 and 2 touching squares (AA, BB), 3 an island astride ±180° split as OSM has it
/// (two rings: 179.9…180, −180…−179.9), 4 a far square.
fn fx() -> Fx {
    let d = tempfile::tempdir().unwrap();
    let p = outlines(
        d.path(),
        &[
            (1, "AA", sq(0.0, 0.0, 1.0)),
            (2, "BB", sq(1.0, 0.0, 1.0)),
            (3, "FJ", vec![vec![[179.9, -17.0], [180.0, -17.0], [180.0, -16.9], [179.9, -16.9], [179.9, -17.0]]]),
            (5, "FJ", vec![vec![[-180.0, -17.0], [-179.9, -17.0], [-179.9, -16.9], [-180.0, -16.9], [-180.0, -17.0]]]),
            (4, "CC", sq(10.0, 10.0, 1.0)),
        ],
    );
    Fx { o: Outlines::open(&p).unwrap(), _d: d }
}

fn folder(files: &[(&str, &str)]) -> resolve::Folder {
    let m: BTreeMap<String, Vec<u8>> = files.iter().map(|(p, t)| (p.to_string(), t.as_bytes().to_vec())).collect();
    resolve::Folder::read(m.keys().cloned(), &|p| Ok(m[p].clone())).unwrap()
}

fn run(f: &Fx, files: &[(&str, &str)]) -> resolve::Outcome {
    let others = BTreeMap::new();
    let ctx = resolve::Context {
        pass: Some(PASS),
        outlines: Some(&f.o),
        extract: None,
        others: &others,
        memo: None,
        names: String::new(),
        data: None,
    };
    resolve::resolve(&folder(files), &ctx)
}

const PINS: (&str, &str) = ("_pins.toml", "osm = \"2026-09-28\"\n");

fn region(id: &str, body: &str) -> String {
    format!("id = \"{id}\"\nname = \"{id}\"\n{body}")
}

#[test]
fn a_plain_region_is_today_s_coverage_exactly() {
    let f = fx();
    let a = region("a", "outline = [\"osm:1\", \"osm:2\"]\n");
    let out = run(&f, &[PINS, ("a.toml", &a)]);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let g = &out.resolved["a"].geometry;
    assert_eq!(
        g.shapes.iter().map(|s| (s.from.as_str(), s.tolerance_m, s.country.as_str())).collect::<Vec<_>>(),
        vec![("osm:1", 1000, "AA"), ("osm:2", 1000, "BB")]
    );
    let new = read::coverage_of(std::slice::from_ref(g));
    // (As the recipes' coverage was made: each `osm:` part its relation's rings, the coast's 1 km,
    // its country.)
    let old = Coverage {
        shapes: [1u64, 2]
            .iter()
            .map(|id| {
                let rec = f.o.by_id(*id).unwrap();
                let mut s = crate::coverage::Shape::new(
                    format!("a: osm:{id}"),
                    f.o.rings(rec).unwrap(),
                    crate::coverage::OSM_BUFFER_M,
                );
                s.country = f.o.country_code(rec);
                s
            })
            .collect(),
        ..Default::default()
    };
    let _: &[Recipe] = &[];
    for b in [
        [0, 0, 5_000_000, 5_000_000],
        [9_000_000, -1_000_000, 11_000_000, 1_000_000],
        [-10_000_000, -10_000_000, 30_000_000, 30_000_000],
    ] {
        assert_eq!(new.fingerprint(b), old.fingerprint(b));
        assert_eq!(new.shapes_key(b), old.shapes_key(b));
    }
    // Stored, and read back, the same; the same input, the same bytes.
    let bytes = g.encode();
    assert_eq!(stored::Geometry::decode(&bytes).unwrap(), *g);
    assert_eq!(run(&f, &[PINS, ("a.toml", &a)]).resolved["a"].geometry.encode(), bytes);
}

#[test]
fn operations_minus_intersect_and_region_refs() {
    let f = fx();
    let both = region("both", "outline = [\"osm:1\", \"osm:2\"]\n");
    // Both countries, minus the second: the first with its 1 km coast, the second taken exactly.
    let a = region("a", "outline = [\"region:both\"]\nminus = [\"osm:2\"]\n");
    let i = region("i", "outline = [\"osm:1\"]\nintersect = [{ buffer = \"osm:2\", by = \"10 km\" }]\n");
    let empty = region("e", "outline = [\"osm:1\"]\nminus = [{ buffer = \"osm:1\", by = \"5 km\" }]\n");
    let out = run(&f, &[PINS, ("both.toml", &both), ("x/a.toml", &a), ("i.toml", &i), ("e.toml", &empty)]);
    let ga = &out.resolved["a"].geometry;
    assert_eq!(ga.shapes.len(), 1);
    assert_eq!(ga.shapes[0].country, "AA");
    let cov = read::coverage_of(std::slice::from_ref(ga));
    assert!(cov.contains([5_000_000, 5_000_000]));
    assert!(cov.contains([-50_000, 5_000_000]), "the first's coast grown by 1 km");
    assert!(!cov.contains([10_100_000, 5_000_000]), "the second taken away");
    let ci = read::coverage_of(std::slice::from_ref(&out.resolved["i"].geometry));
    assert!(ci.contains([9_500_000, 5_000_000]) && !ci.contains([5_000_000, 5_000_000]));
    assert!(out.problems.iter().any(|p| p.check == "rg-empty" && p.about == ["e"]));
}

#[test]
fn loops_refs_dups_and_pins_are_problems() {
    let f = fx();
    let l = |id: &str, to: &str| region(id, &format!("outline = [\"region:{to}\"]\n"));
    let out = run(
        &f,
        &[
            PINS,
            ("a.toml", &l("a", "b")),
            ("b.toml", &l("b", "c")),
            ("c.toml", &l("c", "a")),
            ("d.toml", &l("d", "nobody")),
        ],
    );
    let lp = out.problems.iter().find(|p| p.check == "rg-loop").unwrap();
    assert_eq!(lp.about, vec!["a", "b", "c"]);
    assert!(out.problems.iter().any(|p| p.check == "rg-ref" && p.about[0] == "d"));
    // One id in two folders.
    let a = region("a", "outline = [\"osm:1\"]\n");
    let out = run(&f, &[PINS, ("x/a.toml", &a), ("y/a.toml", &a)]);
    assert_eq!(out.problems[0].check, "rg-dup-id");
    assert_eq!(out.problems[0].files, vec!["x/a.toml", "y/a.toml"]);
    // No pin; a pin naming another pass.
    let out = run(&f, &[("a.toml", &a)]);
    assert_eq!(out.problems[0].check, "rg-pin");
    let out = run(&f, &[("_pins.toml", "osm = \"2026-06-01\"\n"), ("a.toml", &a)]);
    assert_eq!(out.problems[0].check, "rg-pin-mismatch");
    assert!(out.problems[0].message.contains("move the pin"));
    // A relation outside the outline set: the extract's, waited for.
    let x = region("x", "outline = [\"osm:99\"]\n");
    let out = run(&f, &[PINS, ("x.toml", &x)]);
    assert_eq!(out.problems[0].check, "rg-extract");
    assert_eq!(out.needs.iter().map(|i| i.to_string()).collect::<Vec<_>>(), vec!["r99"]);
}

#[test]
fn the_antimeridian_island_buffered_is_two_shapes_within_180() {
    let f = fx();
    // The island (two relations as OSM splits it), 12 nmi round.
    let r = region("fiji", "outline = [{ buffer = \"region:isl\", by = \"12 nmi\" }]\n");
    let isl = region("isl", "outline = [\"osm:3\", \"osm:5\"]\n");
    let out = run(&f, &[PINS, ("fiji.toml", &r), ("isl.toml", &isl)]);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let g = &out.resolved["fiji"].geometry;
    assert_eq!(g.shapes.len(), 2);
    assert!(g.shapes[0].from.ends_with("(to 180°)") && g.shapes[1].from.ends_with("(from -180°)"));
    for s in &g.shapes {
        for p in s.rings.iter().flatten() {
            assert!((-1_800_000_000..=1_800_000_000).contains(&p[0]));
        }
    }
    let cov = read::coverage_of(std::slice::from_ref(g));
    // 10 km past the island on either side of the line, and north across it: in; 40 km off: out.
    assert!(cov.contains([1_798_100_000, -169_500_000]));
    assert!(cov.contains([-1_798_100_000, -169_500_000]));
    assert!(cov.contains([1_799_990_000, -168_200_000]) && cov.contains([-1_799_990_000, -168_200_000]));
    assert!(!cov.contains([-1_795_000_000, -169_500_000]));
    assert!(!cov.fingerprint([1_799_000_000, -171_000_000, 1_800_000_000, -168_000_000]).is_empty());
    assert!(!cov.shapes_key([-1_800_000_000, -171_000_000, -1_799_000_000, -168_000_000]).is_empty());
}

#[test]
fn a_shape_drawn_across_the_line_is_the_shape_split_at_it() {
    let f = fx();
    let across =
        r#"{"type": "Polygon", "coordinates": [[[179.5, 1], [-179.5, 1], [-179.5, 2], [179.5, 2], [179.5, 1]]]}"#;
    let split = r#"{"type": "MultiPolygon", "coordinates": [[[[179.5, 1], [180, 1], [180, 2], [179.5, 2], [179.5, 1]]], [[[-180, 1], [-179.5, 1], [-179.5, 2], [-180, 2], [-180, 1]]]]}"#;
    // Computed (a minus of nothing near), so each is put in one frame and stored canonical.
    let r = region("k", "outline = [\"shape:k.geojson\"]\nminus = [\"osm:4\"]\n");
    let a = run(&f, &[PINS, ("k.toml", &r), ("k.geojson", across)]);
    let b = run(&f, &[PINS, ("k.toml", &r), ("k.geojson", split)]);
    assert!(a.problems.is_empty(), "{:?}", a.problems);
    assert_eq!(a.resolved["k"].geometry, b.resolved["k"].geometry);
    assert_eq!(a.resolved["k"].geometry.shapes.len(), 2);
    // A cut by a line drawn across the line: the north half kept on both sides.
    let c = region("c", "outline = [{ cut = \"shape:k.geojson\", by = \"line:l\", keep = \"left\" }]\n[lines.l]\ncoords = [[179, 1.5], [-179, 1.5]]\n");
    let out = run(&f, &[PINS, ("c.toml", &c), ("k.geojson", across)]);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let cov = read::coverage_of(std::slice::from_ref(&out.resolved["c"].geometry));
    assert!(cov.contains([1_798_000_000, 18_000_000]) && cov.contains([-1_798_000_000, 18_000_000]));
    assert!(!cov.contains([1_798_000_000, 12_000_000]) && !cov.contains([-1_798_000_000, 12_000_000]));
}

#[test]
fn the_gate_takes_regions_in_and_holds_them_on_a_new_pass() {
    let f = fx();
    let checks = Regions::new(Some(Prepared {
        pass: Some(PASS.into()),
        outlines: Some(Outlines::open(&f._d.path().join("outlines.sect")).unwrap()),
        ..Default::default()
    }));
    let a = region("a", "outline = [\"osm:1\"]\n");
    let update: BTreeMap<String, Vec<u8>> = [("_pins.toml", PINS.1), ("eu/a.toml", a.as_str())]
        .iter()
        .map(|(p, t)| (p.to_string(), t.as_bytes().to_vec()))
        .collect();
    let listing = Listing {
        files: update.keys().map(|p| (p.clone(), (update[p].len() as u64, 1))).collect(),
        ..Default::default()
    };
    let accepted = Default::default();
    let unsettled = Default::default();
    let rn = |p: &str| Ok(update[p].clone());
    let ro = |_: &FileEntry| -> Result<Vec<u8>> { unreachable!() };
    let d = decide(&Given {
        checks: &checks,
        prev: None,
        prev_listed: &BTreeMap::new(),
        listing: &listing,
        unsettled: &unsettled,
        accepted: &accepted,
        full: false,
        read_new: &rn,
        read_old: &ro,
    })
    .unwrap();
    assert!(d.report.is_none(), "{:?}", d.report);
    let e = &d.index.files["eu/a.toml"];
    assert!(e.facts["geometry"].as_str().unwrap().starts_with("sources/inputs/regions/@geometry/a."));
    assert_eq!(e.keyed, e.facts["geometry"].as_str().unwrap());
    assert!(d.store.keys().any(|k| k.contains("@drawn/a.")));
    // A new pass: nothing changed, but the region held, its accepted geometry kept.
    let store: BTreeMap<String, Arc<Vec<u8>>> = d.store.clone();
    let prev = d.index.clone();
    let checks2 = Regions::new(Some(Prepared {
        pass: Some("2026-12-28".into()),
        outlines: Some(Outlines::open(&f._d.path().join("outlines.sect")).unwrap()),
        ..Default::default()
    }));
    let ro2 = |e: &FileEntry| -> Result<Vec<u8>> { Ok(store[&e.file].to_vec()) };
    let d2 = decide(&Given {
        checks: &checks2,
        prev: Some(&prev),
        prev_listed: &d.listed,
        listing: &listing,
        unsettled: &unsettled,
        accepted: &accepted,
        full: false,
        read_new: &rn,
        read_old: &ro2,
    })
    .unwrap();
    assert!(!d2.changed);
    let r = d2.report.unwrap();
    assert_eq!(r.held, vec!["eu/a.toml"]);
    assert_eq!(r.findings[0].check(), "rg-pin-mismatch");
    // A file the regions don't take: ignored.
    assert!(
        !checks.takes("notes.txt")
            && !checks.takes("old.poly")
            && !checks.takes("eu/old.toml.removed")
            && checks.takes("x/y.geojson")
    );
    let _: &Index = &prev;
}

/// A new version of the checks derives again with the files unchanged: only the drawn files (and the
/// index's `checks`) change; the stored geometry and the `keyed` digests the builds' keys read stay.
#[test]
fn a_new_checks_version_derives_again_and_changes_only_the_drawn_files() {
    let f = fx();
    let checks = Regions::new(Some(Prepared {
        pass: Some(PASS.into()),
        outlines: Some(Outlines::open(&f._d.path().join("outlines.sect")).unwrap()),
        ..Default::default()
    }));
    let a = region("a", "outline = [\"osm:1\"]\n");
    let update: BTreeMap<String, Vec<u8>> = [("_pins.toml", PINS.1), ("eu/a.toml", a.as_str())]
        .iter()
        .map(|(p, t)| (p.to_string(), t.as_bytes().to_vec()))
        .collect();
    let listing = Listing {
        files: update.keys().map(|p| (p.clone(), (update[p].len() as u64, 1))).collect(),
        ..Default::default()
    };
    let (accepted, unsettled) = (Default::default(), Default::default());
    let rn = |p: &str| Ok(update[p].clone());
    let kept = std::cell::RefCell::new(BTreeMap::<String, Arc<Vec<u8>>>::new());
    let ro = |e: &FileEntry| -> Result<Vec<u8>> { Ok(kept.borrow()[&e.file].to_vec()) };
    let run = |prev: Option<&Index>, listed: &BTreeMap<String, (u64, u64)>| {
        decide(&Given {
            checks: &checks,
            prev,
            prev_listed: listed,
            listing: &listing,
            unsettled: &unsettled,
            accepted: &accepted,
            full: false,
            read_new: &rn,
            read_old: &ro,
        })
        .unwrap()
    };
    let d = run(None, &BTreeMap::new());
    kept.borrow_mut().extend(d.store.clone());
    // The accepted version as an older app made it: its checks version lower, its drawing other.
    let mut old = d.index.clone();
    old.checks = format!("regions {}", checks.version() - 1);
    old.files
        .get_mut("eu/a.toml")
        .unwrap()
        .facts
        .insert("drawn".into(), "sources/inputs/regions/@drawn/a.0.json".into());
    let d2 = run(Some(&old), &d.listed);
    assert!(d2.changed, "not changed");
    assert!(d2.report.as_ref().is_none_or(|r| r.findings.is_empty() && r.held.is_empty()), "{:?}", d2.report);
    assert_eq!(d2.index.checks, d.index.checks);
    let (e, e0) = (&d2.index.files["eu/a.toml"], &old.files["eu/a.toml"]);
    assert_eq!((&e.file, &e.keyed, &e.facts["geometry"]), (&e0.file, &e0.keyed, &e0.facts["geometry"]));
    assert_eq!(e.facts["drawn"], d.index.files["eu/a.toml"].facts["drawn"]);
    assert_ne!(e.facts["drawn"], e0.facts["drawn"]);
    // Taken: it derives no more.
    assert!(!run(Some(&d2.index), &d2.listed).changed);
}

/// A region's geometry is pinned to what it's resolved from and `RESOLUTION` (#832): a change
/// that doesn't touch what a region reads (a folder's `_group.toml`, another region's file) keeps
/// its accepted geometry, even one the resolution as it is now would make otherwise; a change of
/// what it reads, or of `RESOLUTION`, derives it again.
#[test]
fn a_change_that_a_region_doesnt_read_keeps_its_accepted_geometry() {
    let f = fx();
    let checks = Regions::new(Some(Prepared {
        pass: Some(PASS.into()),
        outlines: Some(Outlines::open(&f._d.path().join("outlines.sect")).unwrap()),
        ..Default::default()
    }));
    let (a, b, b2) = (
        region("a", "outline = [\"osm:1\"]\n"),
        region("b", "outline = [\"osm:2\"]\n"),
        region("b", "outline = [\"osm:2\", \"osm:4\"]\n"),
    );
    let files = |extra: &[(&str, &str)]| -> BTreeMap<String, Vec<u8>> {
        [("_pins.toml", PINS.1), ("eu/a.toml", a.as_str())]
            .iter()
            .chain(extra)
            .map(|(p, t)| (p.to_string(), t.as_bytes().to_vec()))
            .collect()
    };
    let (accepted, unsettled) = (Default::default(), Default::default());
    let kept = std::cell::RefCell::new(BTreeMap::<String, Arc<Vec<u8>>>::new());
    let ro = |e: &FileEntry| -> Result<Vec<u8>> { Ok(kept.borrow()[&e.file].to_vec()) };
    let run = |update: &BTreeMap<String, Vec<u8>>, prev: Option<&Index>, listed: &BTreeMap<String, (u64, u64)>| {
        let listing = Listing {
            files: update.keys().map(|p| (p.clone(), (update[p].len() as u64, 1))).collect(),
            ..Default::default()
        };
        let rn = |p: &str| Ok(update[p].clone());
        let d = decide(&Given {
            checks: &checks,
            prev,
            prev_listed: listed,
            listing: &listing,
            unsettled: &unsettled,
            accepted: &accepted,
            full: true,
            read_new: &rn,
            read_old: &ro,
        })
        .unwrap();
        kept.borrow_mut().extend(d.store.clone());
        d
    };
    let d = run(&files(&[("eu/b.toml", &b)]), None, &BTreeMap::new());
    assert!(d.report.is_none(), "{:?}", d.report);
    let e = d.index.files["eu/a.toml"].clone();
    assert!(e.facts["resolved_from"].as_str().unwrap().starts_with(&format!("{RESOLUTION} ")));
    // The accepted geometry as an older resolution made it (same RESOLUTION: a change nobody bumped).
    let fake = "sources/inputs/regions/@geometry/a.0000000000000000.rgeo";
    let mut old = d.index.clone();
    let ea = old.files.get_mut("eu/a.toml").unwrap();
    ea.facts.insert("geometry".into(), fake.into());
    ea.keyed = fake.into();
    // A group's name added and another region edited: a's geometry stays, b's is made again.
    let next = files(&[("eu/b.toml", &b2), ("eu/_group.toml", "name = \"Europe\"\n")]);
    let d2 = run(&next, Some(&old), &d.listed);
    assert!(d2.changed && d2.report.is_none(), "{:?}", d2.report);
    assert_eq!(d2.index.files["eu/a.toml"].keyed, fake);
    assert_eq!(d2.index.files["eu/a.toml"].facts["geometry"], fake);
    assert_ne!(d2.index.files["eu/b.toml"].keyed, d.index.files["eu/b.toml"].keyed);
    // The resolution's version bumped (an index of the version before): a is resolved again.
    let mut older = d2.index.clone();
    older.checks = format!("regions {}", RESOLUTION - 1);
    let ea = older.files.get_mut("eu/a.toml").unwrap();
    ea.facts.insert("resolved_from".into(), format!("{} x", RESOLUTION - 1).into());
    let d3 = run(&next, Some(&older), &d2.listed);
    assert_eq!(d3.index.files["eu/a.toml"].keyed, e.keyed);
    // Its own file changed: resolved again.
    let mut edited = next.clone();
    edited.insert("eu/a.toml".into(), region("a", "outline = [\"osm:1\", \"osm:4\"]\n").into_bytes());
    let d4 = run(&edited, Some(&old), &d.listed);
    assert_ne!(d4.index.files["eu/a.toml"].keyed, fake);
}

#[test]
fn a_line_from_ways_buffered_and_ways_that_dont_join() {
    let f = fx();
    let way = |ends: [u64; 2], line: Vec<[i32; 2]>| Some(extract::Way { ends, line });
    let mut e = extract::Extract { fmt: 1, pass: PASS.into(), ..Default::default() };
    // A river of two ways, the second drawn backwards; a third that meets neither.
    e.ways.insert("11".into(), way([1, 2], vec![[50_000_000, 50_000_000], [51_000_000, 50_000_000]]));
    e.ways.insert("12".into(), way([3, 2], vec![[52_000_000, 50_000_000], [51_000_000, 50_000_000]]));
    e.ways.insert("13".into(), way([8, 9], vec![[60_000_000, 50_000_000], [61_000_000, 50_000_000]]));
    let others = BTreeMap::new();
    let ctx = resolve::Context {
        pass: Some(PASS),
        outlines: Some(&f.o),
        extract: Some(&e),
        others: &others,
        memo: None,
        names: String::new(),
        data: None,
    };
    let r = region("river", "outline = [{ buffer = \"line:r\", by = \"3 km\" }]\n[lines.r]\nways = [11, 12]\n");
    let out = resolve::resolve(&folder(&[PINS, ("river.toml", &r)]), &ctx);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    let cov = read::coverage_of(std::slice::from_ref(&out.resolved["river"].geometry));
    assert!(cov.contains([51_500_000, 50_150_000]), "2 km off the second way: in");
    assert!(!cov.contains([51_500_000, 50_400_000]), "4.5 km off: out");
    let bad = region("bad", "outline = [{ buffer = \"line:r\", by = \"3 km\" }]\n[lines.r]\nways = [11, 13]\n");
    let out = resolve::resolve(&folder(&[PINS, ("bad.toml", &bad)]), &ctx);
    assert!(
        out.problems.iter().any(|p| p.check == "rg-line" && p.message.contains("ways 11 and 13 don't join")),
        "{:?}",
        out.problems
    );
    // A way the extract lacks: waited for, named in the needs.
    let more = region("more", "outline = [{ buffer = \"line:r\", by = \"3 km\" }]\n[lines.r]\nways = [11, 14]\n");
    let out = resolve::resolve(&folder(&[PINS, ("more.toml", &more)]), &ctx);
    assert_eq!(out.needs.iter().map(|i| i.to_string()).collect::<Vec<_>>(), vec!["w14"]);
}

#[test]
fn folders_make_the_tree_and_moving_a_region_changes_no_key() {
    let f = fx();
    let checks = Regions::new(Some(Prepared {
        pass: Some(PASS.into()),
        outlines: Some(Outlines::open(&f._d.path().join("outlines.sect")).unwrap()),
        ..Default::default()
    }));
    let take = |files: &[(&str, &str)]| {
        let update: BTreeMap<String, Vec<u8>> =
            files.iter().map(|(p, t)| (p.to_string(), t.as_bytes().to_vec())).collect();
        let listing = Listing {
            files: update.keys().map(|p| (p.clone(), (update[p].len() as u64, 1))).collect(),
            ..Default::default()
        };
        let rn = |p: &str| Ok(update[p].clone());
        let ro = |_: &FileEntry| -> Result<Vec<u8>> { unreachable!() };
        decide(&Given {
            checks: &checks,
            prev: None,
            prev_listed: &BTreeMap::new(),
            listing: &listing,
            unsettled: &Default::default(),
            accepted: &Default::default(),
            full: false,
            read_new: &rn,
            read_old: &ro,
        })
        .unwrap()
    };
    let (a, b, c) = (
        region("a", "outline = [\"osm:1\"]\n"),
        region("b", "outline = [\"osm:2\"]\n"),
        region("c", "outline = [\"osm:4\"]\n"),
    );
    let d = take(&[
        PINS,
        ("a.toml", &a),
        ("eu/pt/b.toml", &b),
        ("eu/_group.toml", "name = \"Europe / EU\"\norder = 2\n"),
        ("am/c.toml", &c),
        ("empty/_group.toml", "name = \"Nothing\"\n"),
    ]);
    assert!(d.report.is_none(), "{:?}", d.report);
    let t = read::tree(&d.index);
    assert_eq!(t.regions, vec!["a"]);
    // Ordered: the one with an order first; an empty folder isn't in the tree.
    assert_eq!(
        t.folders.iter().map(|f| (f.path.as_str(), f.name.as_str())).collect::<Vec<_>>(),
        vec![("eu", "Europe / EU"), ("am", "am")]
    );
    assert_eq!(t.folders[0].folders[0].path, "eu/pt");
    assert_eq!(t.folders[0].folders[0].regions, vec!["b"]);
    // Moved to another folder: the same geometry, so the same keys.
    let d2 = take(&[PINS, ("a.toml", &a), ("b.toml", &b), ("am/c.toml", &c)]);
    assert_eq!(d.index.files["eu/pt/b.toml"].keyed, d2.index.files["b.toml"].keyed);
    assert_eq!(d.index.files["eu/pt/b.toml"].facts["geometry"], d2.index.files["b.toml"].facts["geometry"]);
    // A bad group file: an error on it.
    assert_eq!(checks.file("eu/_group.toml", b"name = \"x\"\ncolour = 1\n").findings[0].check(), "rg-group");
    assert!(checks.takes_ignored("_group.toml") && checks.takes_ignored("_draft.toml"));
    assert!(!checks.takes_ignored("_x.geojson") && !checks.takes_ignored(".DS_Store"));
}

/// Any `_*.toml` is special, never a region: the known three are read, any other is a fault naming
/// them, the old names are a fault saying what to rename them to, and the special files at the wrong
/// place are faults (#536).
#[test]
fn a_name_starting_with_an_underscore_is_special() {
    use resolve::{is_special, special, Special};
    let checks = Regions::new(None);
    assert!(is_special("_x.toml") && is_special("a/_group.toml") && !is_special("a.toml") && !is_special("_x.geojson"));
    assert_eq!(special("_pins.toml"), Some(Special::Pins));
    assert_eq!(special("_rules.toml"), Some(Special::Rules));
    assert_eq!(special("eu/_group.toml"), Some(Special::Group));
    assert_eq!(special("eu/_pins.toml"), None);
    assert_eq!(special("_draft.toml"), None);
    let ok = |p: &str, t: &str| checks.file(p, t.as_bytes()).findings;
    assert!(ok("_pins.toml", "osm = \"2026-09-28\"\n").is_empty());
    assert!(ok("_rules.toml", "cover = \"earth\"\n").is_empty());
    for (p, t) in [("_draft.toml", ""), ("eu/_pins.toml", "osm = \"2026-09-28\"\n"), ("eu/_rules.toml", "")] {
        let fs = ok(p, t);
        assert_eq!(fs.len(), 1, "{p}");
        assert_eq!(fs[0].check(), "rg-special");
        assert!(
            fs[0].message.contains("_pins.toml")
                && fs[0].message.contains("_rules.toml")
                && fs[0].message.contains("_group.toml")
        );
    }
    for (old, new) in [("pins.toml", "_pins.toml"), ("rules.toml", "_rules.toml")] {
        let fs = ok(old, "osm = \"2026-09-28\"\n");
        assert_eq!(fs.len(), 1, "{old}");
        assert_eq!(fs[0].check(), "rg-special");
        assert!(fs[0].message.contains(&format!("is now {new}")));
    }
    // A region with the file name of a special one is that: a fault, never a region.
    let fs = ok("_a.toml", &region("_a", "outline = [\"osm:1\"]\n"));
    assert_eq!(fs[0].check(), "rg-special");
    // The folder reads them as what they are, and an unknown one is no region.
    let files: BTreeMap<String, Vec<u8>> =
        [("_pins.toml", PINS.1), ("_rules.toml", "cover = \"earth\"\n"), ("_draft.toml", "id = \"x\"\n")]
            .iter()
            .map(|(p, t)| (p.to_string(), t.as_bytes().to_vec()))
            .collect();
    let folder = resolve::Folder::read(files.keys().cloned(), &|p| Ok(files[p].clone())).unwrap();
    assert_eq!(folder.pins.get("osm").map(String::as_str), Some("2026-09-28"));
    assert_eq!(folder.rules.unwrap().cover.as_deref(), Some("earth"));
    assert!(folder.regions.is_empty() && folder.unread.contains("_draft.toml"));
}

/// A NAS root for the check job: the records naming pass `PASS` (and its outline set, when
/// `outlines`), and region `a` in the inputs folder, old enough to be taken.
fn job_root(f: &Fx, outlines: bool) -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let mut m = BTreeMap::new();
    m.insert(format!("sources/osm/{PASS}/pass"), format!("sources/osm/{PASS}/pass.1111111111111111.json"));
    if outlines {
        let name = format!("sources/osm/{PASS}/outlines.2222222222222222.sect");
        std::fs::create_dir_all(root.join(&name).parent().unwrap()).unwrap();
        std::fs::copy(f._d.path().join("outlines.sect"), root.join(&name)).unwrap();
        m.insert(format!("sources/osm/{PASS}/outlines"), name);
    }
    std::fs::create_dir_all(root.join("state/build")).unwrap();
    std::fs::write(root.join("state/build/manifest.json"), serde_json::to_vec(&m).unwrap()).unwrap();
    let dir = root.join("inputs/regions");
    std::fs::create_dir_all(&dir).unwrap();
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    for (p, t) in [PINS, ("a.toml", region("a", "outline = [\"osm:1\"]\n").as_str())] {
        std::fs::write(dir.join(p), t).unwrap();
        std::fs::File::options().write(true).open(dir.join(p)).unwrap().set_modified(old).unwrap();
    }
    d
}

fn manifest(root: &store::nasfs::NasRoot) -> BTreeMap<String, String> {
    crate::out::read_record(&root.join("state/build/manifest.json")).unwrap()
}

#[test]
fn without_the_pass_s_outline_set_the_check_does_nothing() {
    let f = fx();
    let d = job_root(&f, false);
    let before = manifest(&store::nasfs::NasRoot::local_for_test(d.path()));
    let mut out =
        crate::out::Out::unbound(&store::nasfs::NasRoot::local_for_test(d.path()), &d.path().join("scratch")).unwrap();
    let said = crate::inputs::gate::run(&mut out, UNIT, false, None).unwrap();
    assert!(said.contains("not checked now") && said.contains("outline set"), "{said}");
    assert_eq!(
        manifest(&store::nasfs::NasRoot::local_for_test(d.path())),
        before,
        "nothing held, taken in or asked for"
    );
    let checks =
        Regions::default().prepared(&store::nasfs::NasRoot::local_for_test(d.path()), &before).unwrap().unwrap();
    assert!(checks.not_ready().is_some() && checks.needs().is_empty());
    // With it, the region is taken in.
    let d = job_root(&f, true);
    let mut out =
        crate::out::Out::unbound(&store::nasfs::NasRoot::local_for_test(d.path()), &d.path().join("scratch")).unwrap();
    let said = crate::inputs::gate::run(&mut out, UNIT, false, None).unwrap();
    assert!(said.contains("taken in"), "{said}");
}

#[test]
fn a_region_missing_a_fact_is_an_error() {
    let f = fx();
    let d = job_root(&f, true);
    let root = d.path();
    let mut out =
        crate::out::Out::unbound(&store::nasfs::NasRoot::local_for_test(root), &root.join("scratch")).unwrap();
    crate::inputs::gate::run(&mut out, UNIT, true, None).unwrap();
    // (Not switched in these records: the readers refuse, naming the runbook.)
    assert!(format!(
        "{:#}",
        read::source(
            &store::nasfs::NasRoot::local_for_test(root),
            &manifest(&store::nasfs::NasRoot::local_for_test(root))
        )
        .unwrap_err()
    )
    .contains("runbook"));
    let mut a = crate::inputs::open(
        &store::nasfs::NasRoot::local_for_test(root),
        &manifest(&store::nasfs::NasRoot::local_for_test(root)),
        UNIT,
    )
    .unwrap()
    .unwrap();
    assert_eq!(read::recipes(&a).unwrap().len(), 1);
    a.index.files.get_mut("a.toml").unwrap().facts.remove("drawn");
    assert!(read::listed(&a).is_err() && read::recipes(&a).is_err(), "never a region left out");
    // Its drawing without its geometry: as broken.
    let facts = &mut a.index.files.get_mut("a.toml").unwrap().facts;
    let g = facts.remove("geometry").unwrap();
    facts.insert("drawn".into(), g);
    assert!(read::listed(&a).is_err());
}

/// `_rules.toml` taken in beside the regions
/// (its facts the rules, no outline): not a region, so the regions read (21 Oct: the index taking it
/// in stopped the regions' build, "has no outline").
#[test]
fn the_rules_file_is_not_a_region() {
    let index: Index = serde_json::from_str(include_str!("testdata/index-empty-regions.json")).unwrap();
    let d = tempfile::tempdir().unwrap();
    let mut a = crate::inputs::Accepted {
        root: store::nasfs::NasRoot::local_for_test(d.path().to_path_buf()),
        version: "@index.be2033d2b403ccb0.json".into(),
        index,
    };
    let n = read::listed(&a).unwrap().len();
    let mut rules = a.index.files.values().next().unwrap().clone();
    rules.facts = serde_json::from_str(r#"{"rules": {"cover": "earth", "sliver_km2": 50.0}}"#).unwrap();
    for name in ["_rules.toml", "_pins.toml"] {
        a.index.files.insert(name.into(), rules.clone());
    }
    assert_eq!(read::listed(&a).unwrap().len(), n);
}

/// The live index's shape (task #504): a cut of `@index.be2033d2b403ccb0.json` (regions 1,028, rules
/// `cover = "earth"`), whose `*-outside-its-regions` (hand_out_all, every place handed out) resolved
/// to nothing and so have no geometry or drawing in their facts. It reads; they're listed and in the
/// tree, never built.
#[test]
fn the_live_index_with_empty_regions_reads() {
    let index: Index = serde_json::from_str(include_str!("testdata/index-empty-regions.json")).unwrap();
    let d = tempfile::tempdir().unwrap();
    let a = crate::inputs::Accepted {
        root: store::nasfs::NasRoot::local_for_test(d.path().to_path_buf()),
        version: "@index.be2033d2b403ccb0.json".into(),
        index,
    };
    let l = read::listed(&a).unwrap();
    let by = |id: &str| l.iter().find(|x| x.recipe.id == id).unwrap();
    for id in ["bolivia-outside-its-regions", "china-outside-its-regions"] {
        assert!(by(id).stored.is_none() && by(id).disabled, "{id}");
    }
    assert!(by("andorra").stored.is_some() && !by("andorra").disabled);
    assert!(by("beni").stored.is_some() && by("beni").disabled);
    assert_eq!(l.len(), 5);
    assert_eq!(read::recipes(&a).unwrap().iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["andorra", "hong-kong"]);
    let t = read::tree(&a.index);
    let bolivia = t.folders.iter().find(|f| f.path == "bolivia").unwrap();
    assert_eq!(bolivia.regions, ["beni", "bolivia-outside-its-regions"]);
    assert!(read::drawn(
        &store::nasfs::NasRoot::local_for_test(d.path()),
        &a,
        &[by("bolivia-outside-its-regions").recipe.clone()]
    )
    .unwrap()
    .is_empty());
}

#[test]
fn pieces_past_two_bands_have_names_of_their_own() {
    let names: Vec<String> = (0..3).map(|k| resolve::piece_name("x", k, 3)).collect();
    assert_eq!(names, ["x (part 1 of 3)", "x (part 2 of 3)", "x (part 3 of 3)"]);
    assert_eq!(
        (resolve::piece_name("x", 0, 2), resolve::piece_name("x", 1, 2), resolve::piece_name("x", 0, 1)),
        ("x (to 180°)".into(), "x (from -180°)".into(), "x".into())
    );
    // An area over more than a turn of longitude splits into three bands.
    let wide = geom::Area::from_rings(&[vec![
        [-1_900_000_000, 0],
        [4_500_000_000, 0],
        [4_500_000_000, 10],
        [-1_900_000_000, 10],
    ]]);
    let parts = geom::Frame::split(&wide);
    assert_eq!(parts.len(), 3);
}

#[test]
fn the_switch_s_app_fails_closed() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let rd = |n: &str| -> Result<Vec<u8>> { std::fs::read(root.join(n)).with_context(|| n.to_string()) };
    let sw = crate::inputs::switched_logical(UNIT);
    // Not switched: none.
    assert_eq!(read::switch_app(&BTreeMap::new(), &rd).unwrap(), None);
    // Switched, its file there: its app.
    std::fs::write(root.join("sw.json"), br#"{"same": true, "app": "20261015-1200-aaaaaaa"}"#).unwrap();
    let m = BTreeMap::from([(sw.clone(), "sw.json".to_string())]);
    assert_eq!(read::switch_app(&m, &rd).unwrap().as_deref(), Some("20261015-1200-aaaaaaa"));
    // Unreadable, unparsed, or naming no app: an error, never an app every Mac passes.
    let gone = BTreeMap::from([(sw.clone(), "gone.json".to_string())]);
    assert!(read::switch_app(&gone, &rd).is_err());
    std::fs::write(root.join("bad.json"), b"{half").unwrap();
    assert!(read::switch_app(&BTreeMap::from([(sw.clone(), "bad.json".to_string())]), &rd).is_err());
    // Who may lead: not an older app, not while unknown; and the app needed, the newer.
    let ok: std::result::Result<Option<String>, String> = Ok(Some("20261015-1200-aaaaaaa".into()));
    assert!(
        read::may_lead("20261015-1200-aaaaaaa", &ok).is_ok() && read::may_lead("20261016-0000-bbbbbbb", &ok).is_ok()
    );
    assert!(read::may_lead("20261014-0000-ccccccc", &ok).unwrap_err().contains("older"));
    assert!(read::may_lead("20261016-0000-bbbbbbb", &Err("the NAS away".into()))
        .unwrap_err()
        .contains("can't be read"));
    assert!(read::may_lead("20261014-0000-ccccccc", &Ok(None)).is_ok());
    assert_eq!(
        read::needed_app(Some("20261014-0000-ccccccc"), Some("20261015-1200-aaaaaaa")).as_deref(),
        Some("20261015-1200-aaaaaaa")
    );
    assert_eq!(
        read::needed_app(Some("20261016-0000-bbbbbbb"), Some("20261015-1200-aaaaaaa")).as_deref(),
        Some("20261016-0000-bbbbbbb")
    );
    assert_eq!(read::needed_app(None, Some("20261015-1200-aaaaaaa")).as_deref(), Some("20261015-1200-aaaaaaa"));
}

/// Writes region files into a check job's inputs folder, old enough to be taken.
fn drop_files(root: &store::nasfs::NasRoot, files: &[(&str, &str)]) {
    let dir = root.join("inputs/regions");
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    for (p, t) in files {
        dir.join(p).write(t).unwrap();
        dir.join(p).set_modified(old).unwrap();
    }
}

#[test]
fn a_disabled_region_is_checked_and_named_but_never_built() {
    let f = fx();
    // Parsed: the field, false by default; anything but a boolean is an error.
    assert!(file::parse("b.toml", &region("b", "outline = [\"osm:2\"]\ndisabled = true\n")).unwrap().disabled);
    assert!(!file::parse("b.toml", &region("b", "outline = [\"osm:2\"]\n")).unwrap().disabled);
    assert!(file::parse("b.toml", &region("b", "outline = [\"osm:2\"]\ndisabled = \"yes\"\n")).is_err());
    // Resolved like any region, and another may name it.
    let b_off = region("b", "outline = [\"osm:2\"]\ndisabled = true\n");
    let c = region("c", "outline = [\"osm:1\", \"osm:2\"]\nminus = [\"region:b\"]\n");
    let out = run(&f, &[PINS, ("b.toml", &b_off), ("c.toml", &c)]);
    assert!(out.problems.is_empty(), "{:?}", out.problems);
    assert!(out.resolved.contains_key("b") && out.resolved.contains_key("c"));
    let cc = read::coverage_of(std::slice::from_ref(&out.resolved["c"].geometry));
    assert!(cc.contains([5_000_000, 5_000_000]) && !cc.contains([15_000_000, 5_000_000]), "c less the disabled b");

    // Taken in by the gate: stored like the others, the index saying it's disabled.
    let d = job_root(&f, true);
    let root = d.path();
    drop_files(&store::nasfs::NasRoot::local_for_test(root), &[("b.toml", &b_off), ("c.toml", &c)]);
    let mut out =
        crate::out::Out::unbound(&store::nasfs::NasRoot::local_for_test(root), &root.join("scratch")).unwrap();
    let said = crate::inputs::gate::run(&mut out, UNIT, true, None).unwrap();
    assert!(said.contains("taken in"), "{said}");
    let a = crate::inputs::open(
        &store::nasfs::NasRoot::local_for_test(root),
        &manifest(&store::nasfs::NasRoot::local_for_test(root)),
        UNIT,
    )
    .unwrap()
    .unwrap();
    assert_eq!(a.index.files["b.toml"].facts["disabled"], true);
    assert!(a.index.files["b.toml"].facts.contains_key("geometry"), "stored, so others read it");
    assert!(!a.index.files["c.toml"].facts.contains_key("disabled"), "an enabled region's facts as before");
    // Listed, and in the tree (the panel shows it), but not among the regions the build reads, nor
    // in the coverage (units, tiles, catalogues and downloads all come from these).
    let listed = read::listed(&a).unwrap();
    assert_eq!(
        listed.iter().map(|l| (l.recipe.id.as_str(), l.disabled)).collect::<Vec<_>>(),
        [("a", false), ("b", true), ("c", false)]
    );
    assert!(read::tree(&a.index).regions.contains(&"b".to_string()));
    assert_eq!(read::recipes(&a).unwrap().iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["a", "c"]);
    let cov = Coverage::accepted(&store::nasfs::NasRoot::local_for_test(root), &a, None).unwrap();
    let b_box = [10_200_000, 2_000_000, 19_800_000, 8_000_000];
    assert!(cov.contains([5_000_000, 5_000_000]) && !cov.contains([15_000_000, 5_000_000]));
    assert!(!cov.meets_rect(b_box));
    assert!(
        cov.by_region().iter().all(|(id, _)| !id.starts_with("b")),
        "{:?}",
        cov.by_region().iter().map(|(i, _)| i).collect::<Vec<_>>()
    );
    let off_print = cov.fingerprint(b_box);

    // Empty by design (resolved to nothing: no stored files), enabled (c) or disabled (b): read,
    // listed and in the tree (the panel shows it), but neither among the regions built nor in the
    // coverage or the drawing.
    let mut e = a.clone();
    for p in ["b.toml", "c.toml"] {
        let facts = &mut e.index.files.get_mut(p).unwrap().facts;
        facts.remove("geometry");
        facts.remove("drawn");
    }
    let l = read::listed(&e).unwrap();
    assert_eq!(
        l.iter().map(|l| (l.recipe.id.as_str(), l.stored.is_some(), l.disabled)).collect::<Vec<_>>(),
        [("a", true, false), ("b", false, true), ("c", false, false)]
    );
    // The enabled empty one (c) named in a warning, once per version; the disabled one (b) not.
    let w = read::skipped_empty("v-empty-test", &l).unwrap();
    assert!(w.starts_with("warning: v-empty-test:") && w.ends_with(": c"), "{w}");
    assert_eq!(read::skipped_empty("v-empty-test", &l), None);
    assert_eq!(read::skipped_empty("v-none", &l[..2]), None);
    let t = read::tree(&e.index);
    assert!(t.regions.contains(&"b".to_string()) && t.regions.contains(&"c".to_string()));
    assert_eq!(read::recipes(&e).unwrap().iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["a"]);
    let ce = Coverage::accepted(&store::nasfs::NasRoot::local_for_test(root), &e, None).unwrap();
    assert!(ce.contains([5_000_000, 5_000_000]));
    assert!(ce.by_region().iter().all(|(id, _)| id.starts_with("a")));
    let named = |id: &str| Recipe { id: id.into(), name: id.into(), outline: vec![] };
    assert_eq!(
        read::drawn(&store::nasfs::NasRoot::local_for_test(root), &e, &[named("a"), named("c")]).unwrap().len(),
        1,
        "c has no drawing"
    );

    // Enabled: a new version, b in the regions and the coverage, and the jobs over its box keyed anew.
    drop_files(&store::nasfs::NasRoot::local_for_test(root), &[("b.toml", &region("b", "outline = [\"osm:2\"]\n"))]);
    let mut out =
        crate::out::Out::unbound(&store::nasfs::NasRoot::local_for_test(root), &root.join("scratch")).unwrap();
    crate::inputs::gate::run(&mut out, UNIT, true, None).unwrap();
    let a2 = crate::inputs::open(
        &store::nasfs::NasRoot::local_for_test(root),
        &manifest(&store::nasfs::NasRoot::local_for_test(root)),
        UNIT,
    )
    .unwrap()
    .unwrap();
    assert_ne!(a2.version, a.version);
    assert_eq!(a2.index.files["b.toml"].keyed, a.index.files["b.toml"].keyed, "the same geometry");
    assert_eq!(read::recipes(&a2).unwrap().len(), 3);
    let cov2 = Coverage::accepted(&store::nasfs::NasRoot::local_for_test(root), &a2, None).unwrap();
    assert!(cov2.contains([15_000_000, 5_000_000]) && cov2.meets_rect(b_box));
    assert_ne!(cov2.fingerprint(b_box), off_print);
    // A box b doesn't reach keeps its key.
    let far = [100_000_000, 100_000_000, 101_000_000, 101_000_000];
    assert_eq!(cov2.fingerprint(far), cov.fingerprint(far));

    // Disabled again: out of the coverage once more, as when it was first disabled.
    drop_files(&store::nasfs::NasRoot::local_for_test(root), &[("b.toml", &b_off)]);
    let mut out =
        crate::out::Out::unbound(&store::nasfs::NasRoot::local_for_test(root), &root.join("scratch")).unwrap();
    crate::inputs::gate::run(&mut out, UNIT, true, None).unwrap();
    let a3 = crate::inputs::open(
        &store::nasfs::NasRoot::local_for_test(root),
        &manifest(&store::nasfs::NasRoot::local_for_test(root)),
        UNIT,
    )
    .unwrap()
    .unwrap();
    assert_eq!(read::recipes(&a3).unwrap().len(), 2);
    assert_eq!(
        Coverage::accepted(&store::nasfs::NasRoot::local_for_test(root), &a3, None).unwrap().fingerprint(b_box),
        off_print
    );
}
