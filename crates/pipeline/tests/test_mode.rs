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

/// `buildstore::out::testing` is true exactly where pipeline's `cfg(test)` applied: its unit tests.
/// The agent's runtime reads it for its test-time behaviours (the NAS and port guards, its short
/// waits, the panics a test wants; task #916 step 6f3), so a process seeing it true uses a test's
/// stand-ins. It turns true only by a crate's `#[cfg(test)]` start-up constructor (`TEST_START`,
/// calling `go_testing`) or in buildstore's own unit tests, so it is true only in some crate's
/// unit tests; and no crate with a `TEST_START` links pipeline but pipeline itself (server's unit
/// tests, which do link it, have none). Binaries and integration tests: the test above.
#[test]
fn only_pipeline_s_unit_tests_run_its_code_testing() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let crates = root.join("crates");
    let mut starts = Vec::new();
    let mut deps: Vec<(String, Vec<String>)> = Vec::new();
    let mut bad = Vec::new();
    for e in std::fs::read_dir(&crates).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Ok(toml) = std::fs::read_to_string(e.path().join("Cargo.toml")) else { continue };
        // (Every dependency of any kind: a dev-dependency's unit tests link it too.)
        let mut section = String::new();
        let mut on = Vec::new();
        for l in toml.lines() {
            let t = l.trim();
            if t.starts_with('[') {
                section = t.to_string();
                continue;
            }
            if section.ends_with("dependencies]") {
                if let Some(d) = t.split(['=', '.', ' ']).next().filter(|d| !d.is_empty() && !d.starts_with('#')) {
                    on.push(d.to_string());
                }
            }
        }
        deps.push((name.clone(), on));
        // Who calls `go_testing`: only a lib.rs's `#[cfg(test)]` TEST_START.
        let mut files = Vec::new();
        rs_files(&e.path().join("src"), &mut files);
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.iter().position(|l| l.starts_with("static TEST_START: extern \"C\" fn() = {"));
            if let Some(s) = start {
                if !f.ends_with("src/lib.rs") || !lines[s.saturating_sub(4)..s].contains(&"#[cfg(test)]") {
                    bad.push(format!("{}: a TEST_START outside a lib.rs's #[cfg(test)]", f.display()));
                }
                starts.push(name.clone());
            }
            for (i, l) in lines.iter().enumerate() {
                let code = l.split("//").next().unwrap_or("");
                if code.contains("go_testing()") && !code.contains("pub fn go_testing()") {
                    let in_start = start.is_some_and(|s| i > s && i < s + 8);
                    if !in_start {
                        bad.push(format!("{}:{}: go_testing() outside a TEST_START", f.display(), i + 1));
                    }
                }
            }
        }
    }
    // The crates linking pipeline: it, and those depending on them.
    let mut linking = vec!["pipeline".to_string()];
    loop {
        let more: Vec<String> = deps
            .iter()
            .filter(|(n, on)| !linking.contains(n) && on.iter().any(|d| linking.contains(d)))
            .map(|(n, _)| n.clone())
            .collect();
        if more.is_empty() {
            break;
        }
        linking.extend(more);
    }
    assert!(linking.contains(&"server".to_string()), "server links pipeline: {linking:?}");
    assert!(starts.contains(&"pipeline".to_string()), "pipeline's TEST_START: {starts:?}");
    for s in &starts {
        if s != "pipeline" && linking.contains(s) {
            bad.push(format!(
                "{s} has a TEST_START and links pipeline: its unit tests would run pipeline's code testing"
            ));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

fn rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
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
