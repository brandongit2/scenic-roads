//! The agent's parts share nothing mutable but through `crate::agent::bus` (docs/agent-parts.md §3,
//! task #871): no `Mutex<` in crates/pipeline/src/agent/ outside bus.rs and nasread.rs (its own
//! call pools). The files not yet split into parts keep the count they have, listed in `LEFT`, until
//! their migration step lands (§7): a file with more fails here, and so does one with fewer, so the
//! list only shrinks and never goes stale. (A scan of the source's text, its comments and strings
//! blanked.)
//!
//! scans: crates/pipeline/src/agent

mod common;

use common::*;
use std::collections::BTreeMap;

/// Where a `Mutex<` may be: the snapshots' own, and nasread's pools.
const OWN: &[&str] = &["bus.rs", "nasread.rs"];

/// The files with a `Mutex<` still, and how many, until their step lands.
const LEFT: &[(&str, usize)] = &[
    ("build.rs", 1),
    ("builtview.rs", 2),
    ("cataloguesview.rs", 2),
    ("jobs.rs", 2),
    ("live.rs", 6),
    ("memguard.rs", 1),
    ("mod.rs", 2),
    ("pageview.rs", 4),
    ("pool.rs", 1),
    ("settings.rs", 1),
    ("tiles.rs", 1),
];

#[test]
fn no_shared_mutex() {
    let dir = root().join("crates/pipeline/src/agent");
    let mut fs = Vec::new();
    files(&dir, "rs", &mut fs);
    let mut found: BTreeMap<String, usize> = BTreeMap::new();
    for p in fs {
        let rel = p.strip_prefix(&dir).unwrap().to_string_lossy().to_string();
        if OWN.contains(&rel.as_str()) {
            continue;
        }
        let text = std::fs::read_to_string(&p).unwrap();
        let n: usize = text.lines().map(|l| code_of(l).matches("Mutex<").count()).sum();
        if n > 0 {
            found.insert(rel, n);
        }
    }
    let left: BTreeMap<String, usize> = LEFT.iter().map(|(f, n)| (f.to_string(), *n)).collect();
    assert_eq!(
        found, left,
        "a Mutex on shared state outside agent::bus (docs/agent-parts.md §3): publish a bus::Snap or send a message; \
         a file with fewer than listed: lower its count in LEFT"
    );
}
