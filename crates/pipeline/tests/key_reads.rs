//! A key reads only through its context (docs/incremental-planning.md §4.1, Reads beside the
//! arguments; task #1517): no environment, clock, local file or NAS read in the code of the key
//! modules, steps.rs, idents.rs and TerrainTiles' readers (tiles.rs), but in the items `EXEMPT`
//! lists with how many and why. What they call in other crates is held at run time instead: a NAS
//! call or the pool's clock while a key is derived panics in the tests (store::keyread).
//!
//! Each entry fails when it matches nothing (or a count differs), so none can go stale.
//!
//! scans: crates/pipeline/src/agent/steps.rs crates/pipeline/src/agent/idents.rs crates/pipeline/src/agent/tiles.rs

mod common;

use common::*;
use std::collections::BTreeMap;

/// The modules whose code derives keys.
const FILES: &[&str] = &[
    "crates/pipeline/src/agent/steps.rs",
    "crates/pipeline/src/agent/idents.rs",
    "crates/pipeline/src/agent/tiles.rs",
];

/// What reads beside a key's context: the environment, the clock, a local file, the NAS.
const READS: &[&str] = &[
    "env::var",
    "var_os",
    "SystemTime",
    "UNIX_EPOCH",
    "now_s",
    "Instant::now",
    "read_dir",
    "std::fs::",
    ".read()",
    "::open(",
    ".open_plain(",
];

/// Items reading so: (file, item, how many, why).
const EXEMPT: &[(&str, &str, usize, &str)] = &[
    // The plan's loads (`&mut self`: no key's context holds TerrainTiles but shared), outside every key.
    (
        "crates/pipeline/src/agent/tiles.rs",
        "load_elevation",
        2,
        "the plan's load, before its keys: the declared elevation datasets' index",
    ),
    (
        "crates/pipeline/src/agent/tiles.rs",
        "load_enclosed",
        1,
        "the plan's load, before its keys: the enclosed seas read once a content name",
    ),
    (
        "crates/pipeline/src/agent/tiles.rs",
        "load_asked",
        4,
        "the plan's load, before its keys: the records' heads, kept here",
    ),
    (
        "crates/pipeline/src/agent/tiles.rs",
        "load_water",
        3,
        "the plan's load, before its keys: the water digests, kept here",
    ),
    (
        "crates/pipeline/src/agent/tiles.rs",
        "tidy",
        3,
        "the plan's load: kept files no manifest names, gone after a while",
    ),
    (
        "crates/pipeline/src/agent/tiles.rs",
        "read",
        3,
        "the plan's load: a pack's index, from where it's kept, else the NAS",
    ),
    // The two readers a key reads the NAS through, by a content name it read in the manifest first.
    (
        "crates/pipeline/src/agent/tiles.rs",
        "target_asked",
        1,
        "a target's record by its content name (store::keyread::recorded)",
    ),
    (
        "crates/pipeline/src/agent/tiles.rs",
        "outlines",
        1,
        "a question's outlines by their content name (store::keyread::recorded)",
    ),
];

#[test]
fn the_key_modules_read_nothing_beside_a_key_s_context() {
    let root = root();
    let mut found: BTreeMap<(String, String), (usize, Vec<String>)> = BTreeMap::new();
    for f in FILES {
        let text = std::fs::read_to_string(root.join(f)).unwrap();
        let code: Vec<String> = text.lines().map(code_of).collect();
        let test = test_lines(&code);
        for (i, l) in code.iter().enumerate() {
            if test[i] {
                continue;
            }
            let n = READS.iter().map(|r| l.matches(r).count()).sum::<usize>();
            if n == 0 {
                continue;
            }
            let item = enclosing_rs(&code, i).map_or_else(|| "(top level)".to_string(), |(name, _)| name);
            let e = found.entry((f.to_string(), item)).or_default();
            e.0 += n;
            e.1.push(format!("{f}:{}: {}", i + 1, l.trim()));
        }
    }
    let mut bad = Vec::new();
    for ((f, item), (n, lines)) in &found {
        match EXEMPT.iter().find(|e| e.0 == f && e.1 == item) {
            Some(e) if e.2 == *n => {}
            Some(e) => bad.push(format!("{f} {item}: {n} reads, EXEMPT says {}:\n  {}", e.2, lines.join("\n  "))),
            None => bad.push(format!("{f} {item}: reads beside a key's context:\n  {}", lines.join("\n  "))),
        }
    }
    for e in EXEMPT {
        if !found.contains_key(&(e.0.to_string(), e.1.to_string())) {
            bad.push(format!("EXEMPT's {} {} matches nothing: drop it", e.0, e.1));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
