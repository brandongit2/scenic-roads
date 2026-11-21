//! The steps reading the shared cache, agentcore::needs::CACHES (task #1173), held to the code
//! that opens it: the whole local copies (buildstore::localread::Local) are opened by
//! `Local::from_env`, and in scenic-build a step's job opens them in its own function, the one its
//! dispatch arm calls. So the steps whose functions call it are the cache's steps, exactly: a step
//! opening the copies unlisted fails here, as does one listed that no longer opens them. Outside
//! scenic-build, `Local::from_env` is called only in the files `ELSEWHERE` names, each with why
//! what it opens there isn't the shared cache (buildstore::localread::is_shared); a new caller
//! fails here until it's a step's (and listed) or named with its reason.
//!
//! scans: crates

mod common;

use std::collections::{BTreeMap, BTreeSet};

/// The call that opens the copies.
const OPENS: &str = "localread::Local::from_env(";

/// The files outside scenic-build that open the copies, with why none of what they read through
/// them is the shared cache.
const ELSEWHERE: &[(&str, &str)] = &[
    (
        "crates/terrain/src/terrain_base.rs",
        "the terrain jobs' stored GeoTIFFs (path_sized): each terrain job's own disk, not the shared cache",
    ),
    (
        "crates/inputs/src/inputs/mod.rs",
        "the inputs' content-named files (read_content): a cheap cache room-making frees",
    ),
];

/// Each function scenic-build's step dispatch calls, by name, with the steps whose arms call it
/// (`"a" | "b" => f(…)`).
fn dispatched(code: &[String]) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for l in code {
        let Some((pats, call)) = l.trim().split_once("=>") else { continue };
        let steps: Vec<&str> = pats.split('|').map(str::trim).collect();
        if steps.is_empty() || !steps.iter().all(|s| s.len() > 2 && s.starts_with('"') && s.ends_with('"')) {
            continue;
        }
        let f: String = call.trim().chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        if f.is_empty() || !call.trim()[f.len()..].starts_with('(') {
            continue;
        }
        out.entry(f).or_default().extend(steps.iter().map(|s| s.trim_matches('"').to_string()));
    }
    out
}

#[test]
fn the_shared_caches_steps_are_those_whose_jobs_open_it() {
    let root = common::root();
    let path = root.join("crates/pipeline/src/bin/scenic-build.rs");
    let text = std::fs::read_to_string(&path).unwrap();
    // (Its lines with the strings' contents kept: the dispatch's patterns are strings.)
    let raw: Vec<String> = text.lines().map(str::to_string).collect();
    let code: Vec<String> = raw.iter().map(|l| common::code_of(l)).collect();
    let tests = common::test_lines(&code);
    let calls = dispatched(&raw);
    let mut opening = BTreeSet::new();
    for (k, l) in code.iter().enumerate().filter(|(k, _)| !tests[*k]) {
        let Some(f) = l.trim().strip_prefix("fn ").map(|r| r.split(['(', '<']).next().unwrap_or("").to_string()) else {
            continue;
        };
        let Some(steps) = calls.get(&f) else { continue };
        let end = common::block_end(&code, k);
        if code[k..=end].iter().any(|l| l.contains(OPENS)) {
            opening.extend(steps.iter().cloned());
        }
    }
    assert!(calls.contains_key("depth_step"), "the dispatch read: {calls:?}");
    let listed: BTreeSet<String> =
        agentcore::needs::CACHES.iter().flat_map(|(_, steps)| steps.iter().map(|s| s.to_string())).collect();
    assert_eq!(
        opening, listed,
        "agentcore::needs::CACHES lists the steps whose jobs open the whole copies (scenic-build's step functions \
         calling {OPENS}…): make it these"
    );
}

#[test]
fn the_copies_are_opened_elsewhere_only_for_what_isnt_the_shared_cache() {
    let root = common::root();
    let mut rs = Vec::new();
    common::files(&root.join("crates"), "rs", &mut rs);
    let mut found = BTreeSet::new();
    for f in rs {
        let rel = f.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        // (A module of tests in its own file, `tests.rs` or `<name>_tests.rs`, is a test's.)
        let test_file = rel.ends_with("/tests.rs") || rel.ends_with("_tests.rs");
        if test_file || rel == "crates/pipeline/src/bin/scenic-build.rs" || rel == "crates/buildstore/src/localread.rs"
        {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap();
        let code: Vec<String> = text.lines().map(common::code_of).collect();
        let tests = common::test_lines(&code);
        if code.iter().enumerate().any(|(k, l)| !tests[k] && l.contains(OPENS)) {
            found.insert(rel);
        }
    }
    let named: BTreeSet<String> = ELSEWHERE.iter().map(|(f, _)| f.to_string()).collect();
    assert_eq!(
        found, named,
        "{OPENS}…) outside scenic-build's steps: a step's job opening the shared cache belongs in its step function \
         (and agentcore::needs::CACHES); a reader of other copies goes in ELSEWHERE with why"
    );
}
