//! Pipeline's library (and the runtime's, agentcore's, under it) as its binaries, examples and
//! integration tests link it: built with its dev-dependencies' features (buildstore's and
//! agentcore's `testkit`) but without `cfg(test)`, so with none of the unit tests' behaviour: `Out`'s retries and the NAS calls' deadline are the real ones, and fetch reaches
//! the network (task #916 step 4 review: `testkit` had made an example offline). And only
//! dev-dependencies turn a `testkit` on, so no release build has one (task #916 step 6d review).
//!
//! scans: crates

#[test]
fn a_build_with_the_dev_dependencies_has_no_test_time_behaviour() {
    assert!(!buildstore::out::testing());
    assert_eq!(buildstore::out::nas_call_deadline(buildstore::out::testing()).as_secs(), 180);
    // (The environment is this test's: the binary's other reads only files.)
    std::env::remove_var("SCENIC_FETCH_OFFLINE");
    assert!(pipeline::fetch::online("https://example.org/x").is_ok());
}

/// `buildstore::out::testing` is true exactly where the runtime's `cfg(test)` applied when it was
/// pipeline's: the unit tests of agentcore (the runtime, task #916 step 6f4) and of pipeline (the
/// plan and the families' tests, which run the runtime). The runtime reads it for its test-time
/// behaviours (the NAS and port guards, its short waits, the panics a test wants; step 6f3), so a
/// process seeing it true uses a test's stand-ins. Its flag is written only by `go_testing` (no
/// environment variable, file or argument reads it), which is named only in a crate's `#[cfg(test)]`
/// start-up constructor (`TEST_START`): in any file of a crate (its library, binaries, tests,
/// examples, build script); else it is true only in buildstore's own unit tests, its initial value.
/// So it is true only in some crate's unit tests; and no crate with a `TEST_START` links agentcore
/// but agentcore and pipeline (server's unit tests, which do link both, have none). Binaries and
/// integration tests: the test above.
#[test]
fn only_the_runtime_s_and_pipeline_s_unit_tests_run_testing() {
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
        // Who names `go_testing`: only a lib.rs's `#[cfg(test)]` TEST_START, in any of the crate's
        // files; and buildstore's flag is written by it alone.
        let mut files = Vec::new();
        rs_files(&e.path(), &mut files);
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
            let out_rs = f.ends_with("buildstore/src/out.rs");
            for (i, l) in lines.iter().enumerate() {
                let code = code_of(l);
                let defined = out_rs && code.trim() == "pub fn go_testing() {";
                let in_start = start.is_some_and(|s| i > s && i < s + 8);
                if names(&code, "go_testing") && !defined && !in_start {
                    bad.push(format!("{}:{}: go_testing named outside a TEST_START", f.display(), i + 1));
                }
                // (The flag itself: private to out.rs, read by `testing`, written by `go_testing`.)
                let flag = [
                    "TESTING.load(std::sync::atomic::Ordering::Relaxed)",
                    "TESTING.store(true, std::sync::atomic::Ordering::Relaxed);",
                ];
                // (Its initial value: true only in buildstore's own unit tests.)
                let declared = out_rs && code == "static TESTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(cfg!(test));";
                if name == "buildstore"
                    && names(&code, "TESTING")
                    && !declared
                    && !(out_rs && flag.contains(&code.trim()))
                {
                    bad.push(format!(
                        "{}:{}: buildstore's TESTING used outside testing/go_testing",
                        f.display(),
                        i + 1
                    ));
                }
            }
        }
    }
    // The crates linking the runtime: agentcore, and those depending on them.
    let mut linking = vec!["agentcore".to_string()];
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
    assert!(linking.contains(&"server".to_string()), "server links agentcore: {linking:?}");
    for c in ["agentcore", "pipeline"] {
        assert!(starts.iter().any(|s| s == c), "{c}'s TEST_START: {starts:?}");
    }
    for s in &starts {
        if !["agentcore", "pipeline"].contains(&s.as_str()) && linking.contains(s) {
            bad.push(format!(
                "{s} has a TEST_START and links agentcore: its unit tests would run the runtime's code testing"
            ));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// Line `l`'s code: its string literals' contents and its comment left out.
fn code_of(l: &str) -> String {
    let (mut out, mut quoted, mut escaped) = (String::new(), false, false);
    let mut chars = l.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => {
                    quoted = false;
                    out.push('"');
                }
                _ => {}
            }
            continue;
        }
        if c == '/' && chars.peek() == Some(&'/') {
            break;
        }
        // (A quote as a character, `'"'`, starts no string.)
        if c == '\'' && chars.peek() == Some(&'"') {
            out.push(c);
            out.push(chars.next().unwrap());
            continue;
        }
        quoted = c == '"';
        out.push(c);
    }
    out
}

/// Whether `code` names identifier `id` (not as part of a longer one).
fn names(code: &str, id: &str) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    code.match_indices(id).any(|(i, _)| {
        !code[..i].chars().next_back().is_some_and(word) && !code[i + id.len()..].chars().next().is_some_and(word)
    })
}

fn rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.file_name().is_some_and(|n| n == "target") {
            continue;
        }
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
