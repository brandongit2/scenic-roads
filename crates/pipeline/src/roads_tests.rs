//! The roads family's tests that open a step's `Out` by the steps table's write sets
//! (crate::out::open): the roads crate's own tests can't (a family doesn't depend on pipeline;
//! docs/crates.md §6.2), so they're pipeline's, as inputs_tests are.
#![cfg(test)]

use inputs::inputs::gtfs::{FERRY_LINES, RAIL_LINES};
use roads::timetables::{run, REPORT, TODO_FILE};

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
    let r = run(&mut out, &cov, &store::nasfs::NasRoot::local_for_test(&root)).unwrap();
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
