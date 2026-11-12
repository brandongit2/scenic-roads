//! The roads family's tests that open a step's `Out` by the steps table's write sets
//! (crate::out::open): the roads crate's own tests can't (a family doesn't depend on pipeline;
//! docs/crates.md §6.2), so they're pipeline's, as inputs_tests are.
#![cfg(test)]

use inputs::inputs::gtfs::{FERRY_LINES, RAIL_LINES, REPORT};
use roads::timetables::{run, TODO_FILE};

#[test]
fn the_job_writes_into_todo_and_puts_the_report() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("root");
    // (The pieces, each its job's.)
    let rail = br#"{"lines": [{"osm": "r1", "kind": "rail", "km": 5.0, "served_km": 0.0, "passenger": true}], "unmatched": []}"#;
    for (step, l, v) in
        [("rail", RAIL_LINES, &rail[..]), ("ferries", FERRY_LINES, &br#"{"lines": [], "unmatched": []}"#[..])]
    {
        let mut o =
            crate::out::open(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("scratch"), step).unwrap();
        o.put_bytes(l, "json", v).unwrap();
        o.save().unwrap();
    }
    let mut out =
        crate::out::open(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("scratch"), "timetables-todo")
            .unwrap();
    std::fs::write(d.path().join("sq.poly"), "sq\n1\n 0 0\n 1 0\n 1 1\n 0 1\nEND\nEND\n").unwrap();
    let cov = buildstore::coverage::Coverage::fixture(
        &[buildstore::recipes::Recipe { id: "s".into(), name: "S".into(), outline: vec!["poly:sq.poly".into()] }],
        None,
        d.path(),
    )
    .unwrap();
    let asking = buildstore::coverage::Asking::new(&cov);
    let r = run(&mut out, &asking, &store::nasfs::NasRoot::local_for_test(&root)).unwrap();
    assert_eq!(r.rail, 1);
    let todo = root.join("inputs/timetables/gtfs/todo");
    assert_eq!(std::fs::read_to_string(todo.join(TODO_FILE)).unwrap().lines().count(), 1);
    assert!(todo.join("README.md").exists());
    assert!(out.get(REPORT).is_some());
    // The inputs folder's listing never takes todo/ in, nor counts it as a change.
    std::fs::write(root.join("inputs/timetables/gtfs/a.zip"), b"z").unwrap();
    for recursive in [false, true] {
        let listed =
            inputs::inputs::list(&store::nasfs::NasRoot::local_for_test(&root), inputs::inputs::gtfs::UNIT, recursive)
                .unwrap();
        assert_eq!(listed.files.keys().collect::<Vec<_>>(), vec!["a.zip"], "{listed:?}");
        assert!(listed.strays.is_empty(), "{listed:?}");
    }
}

/// Task #1130: timetables-todo, keyed on what its run asked of the coverage, from run to plan and
/// back: its run puts its record through its `Out`, the plan reads it from the NAS
/// (`TerrainTiles::load`) and works out the key the run was dispatched under (one run settles it);
/// a region added far from every point it asked keeps that key, one grown over a point it asked
/// changes it, and the run under the new key settles that one too.
#[test]
fn timetables_todo_reruns_only_where_a_region_change_changes_an_answer() {
    use crate::agent::{build::KeyCtx, steps, tiles::TerrainTiles};
    use buildstore::coverage::Coverage;
    let d = tempfile::tempdir().unwrap();
    let root = store::nasfs::NasRoot::local_for_test(&d.path().join("root"));
    let scratch = d.path().join("scratch");
    // A ferry line with an end in Reykjavik, one with its ends 25 and 60 km east of it.
    let ferries = br#"{"lines": [
        {"osm": "w1", "kind": "ferry", "km": 3.0, "ends": [[-21.9, 64.13], [-21.95, 64.2]]},
        {"osm": "w2", "kind": "ferry", "km": 18.0, "ends": [[-21.386, 64.13], [-20.666, 64.13]]}
    ], "unmatched": []}"#;
    let rail = br#"{"lines": [], "unmatched": []}"#;
    for (step, l, v) in [("rail", RAIL_LINES, &rail[..]), ("ferries", FERRY_LINES, &ferries[..])] {
        let mut o = crate::out::open(&root, &scratch, step).unwrap();
        o.put_bytes(l, "json", v).unwrap();
        o.save().unwrap();
    }
    let cov_of = |places: &[&str]| {
        let recipes: Vec<buildstore::recipes::Recipe> = places
            .iter()
            .enumerate()
            .map(|(i, p)| buildstore::recipes::Recipe {
                id: format!("r{i}"),
                name: format!("R{i}"),
                outline: vec![format!("place:{p}")],
            })
            .collect();
        Coverage::fixture(&recipes, None, d.path()).unwrap()
    };
    let reykjavik = cov_of(&["-21.9,64.13,20"]);
    let far = cov_of(&["-21.9,64.13,20", "-45,70,100"]);
    let near = cov_of(&["-21.9,64.13,30"]);
    let inputs = std::collections::BTreeMap::new();
    let mut tiles = TerrainTiles::new(None);
    let mut plan = |cov: &Coverage| {
        let m = crate::out::open(&root, &scratch, "timetables-todo").unwrap().manifest;
        tiles.load(&root, &m);
        steps::key("timetables-todo", "timetables-todo", &KeyCtx::new(cov, "d", &m, &inputs, None, &tiles)).unwrap()
    };
    let run_job = |cov: &Coverage| {
        let mut out = crate::out::open(&root, &scratch, "timetables-todo").unwrap();
        let asked = buildstore::asked::Run::start(&out, "timetables-todo", cov, &|_| None).unwrap();
        let r = run(&mut out, asked.asking(), &root).unwrap();
        asked.put(&mut out, Vec::new()).unwrap();
        r.ferry
    };
    let first = plan(&reykjavik);
    assert_eq!(run_job(&reykjavik), 1);
    assert_eq!(plan(&reykjavik), first, "the key it ran under is the plan's after it");
    assert_eq!(plan(&far), first, "a region far from every point asked keeps it");
    let grown = plan(&near);
    assert_ne!(grown, first, "a region grown over a point asked changes it");
    assert_eq!(run_job(&near), 2);
    assert_eq!(plan(&near), grown, "one run settles it");
    assert_ne!(plan(&reykjavik), grown, "and back again, it reruns");
}
