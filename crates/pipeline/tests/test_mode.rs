//! Pipeline's library as its binaries, examples and integration tests link it: built with its
//! dev-dependencies' features (buildstore's `testkit`) but without `cfg(test)`, so with none of the
//! unit tests' behaviour: `Out`'s retries and verify's deadline are the real ones, and fetch reaches
//! the network (task #916 step 4 review: `testkit` had made an example offline).

#[test]
fn a_build_with_the_dev_dependencies_has_no_test_time_behaviour() {
    assert!(!buildstore::out::testing());
    assert_eq!(buildstore::out::verify_deadline(0, buildstore::out::testing()).as_secs(), 180);
    // (This binary's one test: the environment is its own.)
    std::env::remove_var("SCENIC_FETCH_OFFLINE");
    assert!(pipeline::fetch::online("https://example.org/x").is_ok());
}
