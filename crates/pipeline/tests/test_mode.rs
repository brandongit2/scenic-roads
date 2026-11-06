//! Pipeline's library as its binaries, examples and integration tests link it: built with its
//! dev-dependencies' features (buildstore's `testkit`) but without `cfg(test)`, so with none of the
//! unit tests' behaviour: `Out`'s retries and verify's deadline are the real ones, and fetch reaches
//! the network (task #916 step 4 review: `testkit` had made an example offline). And only
//! dev-dependencies turn a `testkit` on, so no release build has one (task #916 step 6d review).
//!
//! scans: crates

#[test]
fn a_build_with_the_dev_dependencies_has_no_test_time_behaviour() {
    assert!(!buildstore::out::testing());
    assert_eq!(buildstore::out::verify_deadline(0, buildstore::out::testing()).as_secs(), 180);
    // (The environment is this test's: the binary's other reads only files.)
    std::env::remove_var("SCENIC_FETCH_OFFLINE");
    assert!(pipeline::fetch::online("https://example.org/x").is_ok());
}

/// No crate's own dependencies turn a `testkit` feature on (nor a default feature): only
/// dev-dependencies do, so a release build (publish's `cargo build --release -p server -p pipeline`)
/// links none of the helpers, and the scans may take `cfg(any(test, feature = "testkit"))` items as
/// test code (tests/common's test_lines).
#[test]
fn only_dev_dependencies_turn_testkit_on() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut bad = Vec::new();
    for e in std::fs::read_dir(root.join("crates")).unwrap().flatten() {
        let Ok(toml) = std::fs::read_to_string(e.path().join("Cargo.toml")) else { continue };
        let mut section = String::new();
        for (n, l) in toml.lines().enumerate() {
            let t = l.trim();
            if t.starts_with('[') {
                section = t.to_string();
                continue;
            }
            let code = t.split('#').next().unwrap_or("");
            let dev = section.ends_with("dev-dependencies]");
            let own = section == "[features]" && code.starts_with("testkit ");
            if code.contains("testkit") && !dev && !own {
                bad.push(format!("{}/Cargo.toml:{} {section}: {t}", e.file_name().to_string_lossy(), n + 1));
            }
        }
    }
    assert!(bad.is_empty(), "testkit turned on outside dev-dependencies:\n{}", bad.join("\n"));
}
