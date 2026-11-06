//! The names family's tests that open a step's `Out` by the steps table's write sets
//! (crate::out::open): the landmarks crate's own tests can't (a family doesn't depend on pipeline;
//! docs/crates.md §6.2), so they're pipeline's, as inputs_tests are.
#![cfg(test)]

use crate::ovconv::testkit::drop_stale;
use buildstore::out::Out;
use std::collections::BTreeMap;

/// A step's sweep of its stale packs removes only names its outputs function allows: a legacy
/// or malformed name under its prefix stays, and the save goes through, rather than fail every
/// run of the step for good.
#[test]
fn a_stale_sweep_removes_only_its_steps_outputs() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("root");
    let mut seed = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("s0")).unwrap();
    for l in ["layers/stations/hi/6-1-2", "layers/stations/hi/6-1-3", "layers/stations/hi/7-1-2", "layers/stations/old"]
    {
        seed.set(l, Some(format!("{l}.0123456789abcdef.pack")));
    }
    seed.save().unwrap();
    let mut out =
        crate::out::open(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("s1"), "stations").unwrap();
    assert_eq!(drop_stale(&mut out, "stations", &["layers/stations/hi/6-1-2".to_string()]), 1);
    out.save().unwrap();
    let m: BTreeMap<String, String> =
        buildstore::out::read_record(&store::nasfs::NasPath::local_for_test(&root.join("state/build/manifest.json")))
            .unwrap();
    assert_eq!(
        m.keys().map(String::as_str).collect::<Vec<_>>(),
        ["layers/stations/hi/6-1-2", "layers/stations/hi/7-1-2", "layers/stations/old"]
    );
}
