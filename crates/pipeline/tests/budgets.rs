//! Every request a job makes to Wikimedia goes through its agent's proxy, taken from the pool's one
//! budget (docs/pool.md §7.7, §12 phase 4's seventh batch; crate::budget). The proxy and the sandbox
//! enforce it at egress whatever a step's code says (with the budgets on, a job has no other way
//! out); this lint, best-effort (a scan of the source's text) and run by tools/app/publish.sh, keeps
//! the steps honest besides:
//!
//! - every file of the jobs' code (the crates' Rust, dem/'s Python) that names a metered host is
//!   listed in `ASKS` with how its requests go (through the proxy, a client honouring
//!   `HTTPS_PROXY`), or that it asks nothing (a step that sees a 429 tells the proxy:
//!   crate::budget::seen, so the budget holds every process and Mac);
//! - nothing turns the proxy off (`.proxy(None)`, `ProxyHandler({})`, curl's `--noproxy`, a
//!   `no_proxy` of its own) but what `BYPASS` lists, with why.
//!
//! A step asking a declared source (docs/inputs.md §6.9) by role (`import declared`,
//! `declared.url(`: dem/declared.py) names no host to scan, so it counts as asking one: it's listed
//! too, whichever host its declaration names (the proxy holds a metered one at egress all the same).
//! A new file naming a metered host, or a new way around the proxy, fails here until it's listed;
//! an entry matching nothing fails too, so the lists can't go stale.

mod common;

use common::*;
use pipeline::budget::{QUERY_DOMAINS, QUERY_HOSTS, WIKIMEDIA};

/// The files of the jobs' code naming a metered host: how their requests go.
const ASKS: &[(&str, &str)] = &[
    ("crates/pipeline/src/budget/mod.rs", "the budget itself: its lists of the metered hosts"),
    ("crates/pipeline/src/budget/proxy.rs", "the proxy's tests, a metered host answered here"),
    ("crates/pipeline/src/budget/tests.rs", "the budget's tests"),
    ("crates/pipeline/src/agent/jobs.rs", "a test: a sandboxed job's request through the proxy"),
    ("crates/pipeline/src/inputs/sources.rs", "its tests: the staged declarations' URLs, compared, asked of no one"),
    ("crates/pipeline/src/wikiask.rs", "ureq (honours HTTPS_PROXY: Proxy::try_from_env); a 429 told (budget::seen)"),
    (
        "crates/pipeline/src/heritagewd.rs",
        "names Wikidata's links and Wikipedia's sites in its queries, asked through crate::wikiask",
    ),
    ("crates/pipeline/src/areadetails.rs", "names English Wikipedia's site in its query, asked through crate::wikiask"),
    (
        "crates/pipeline/src/items.rs",
        "names Wikidata's entities and English Wikipedia's site in its queries, asked through crate::wikiask",
    ),
    (
        "dem/bldfetch.py",
        "the declared buildings and building-heights sources (Overture's bucket, GHSL's tiles), not metered",
    ),
    ("dem/buildings.py", "the declared buildings source's bucket (DuckDB), not metered"),
    ("dem/bldprep.py", "the declared building-heights source's version alone (its folder): asks nothing"),
];

/// What turns the proxy off, in a line.
const OFF: &[&str] = &[".proxy(None)", "ProxyHandler({})", "--noproxy", "no_proxy", "NO_PROXY"];

/// The lines turning the proxy off: by file and what the line says, with why.
const BYPASS: &[(&str, &str, &str)] = &[
    (
        "crates/pipeline/src/bin/scenic-metrics.rs",
        ".proxy(None)",
        "the owner's metrics tool, run by hand on this Mac, asking the agents here and on the LAN: never a job",
    ),
    (
        "crates/pipeline/src/budget/mod.rs",
        ".proxy(None)",
        "the hold and the retries told to the proxy itself, on this Mac (budget::seen, budget::retried)",
    ),
    (
        "crates/pipeline/src/budget/proxy.rs",
        "NO_PROXY",
        "the job's own Mac (its coordinator on 127.0.0.1) direct: the sandbox allows it",
    ),
    (
        "crates/pipeline/src/agent/mod.rs",
        "NO_PROXY=localhost,127.0.0.1,::1",
        "a test: the job's environment as the proxy gives it",
    ),
];

/// Whether `text` names a metered host (in a URL, a query's prefix, a string).
fn names_metered(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    QUERY_HOSTS.iter().chain(QUERY_DOMAINS.iter()).chain(WIKIMEDIA.iter()).any(|d| {
        t.match_indices(d).any(|(i, _)| {
            // (A domain, not a longer word ending so: what comes before is a dot, a slash, a quote, a
            // space or the line's start.)
            i == 0 || matches!(t.as_bytes()[i - 1], b'.' | b'/' | b'"' | b'\'' | b' ' | b'(' | b'[' | b'@')
        })
    })
}

/// Whether `text` asks a declared source by role (dem/declared.py): a host no scan can see.
fn asks_declared(text: &str) -> bool {
    text.contains("import declared") || text.contains("declared.url(")
}

fn jobs_code() -> Vec<(String, String)> {
    let root = root();
    let mut fs = Vec::new();
    files(&root.join("crates"), "rs", &mut fs);
    files(&root.join("dem"), "py", &mut fs);
    let mut out: Vec<(String, String)> = fs
        .into_iter()
        .map(|f| {
            (
                f.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"),
                std::fs::read_to_string(&f).unwrap_or_default(),
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
fn every_file_naming_wikimedia_is_listed_with_how_it_asks() {
    let mut bad = Vec::new();
    let mut used = std::collections::BTreeSet::new();
    for (rel, text) in jobs_code() {
        // (This lint's own file names them: it isn't in `files`' walk, which skips tests/.)
        if !names_metered(&text) && !asks_declared(&text) {
            continue;
        }
        match ASKS.iter().position(|(f, _)| *f == rel) {
            Some(i) => {
                used.insert(i);
            }
            None => bad.push(rel),
        }
    }
    let stale: Vec<_> = ASKS.iter().enumerate().filter(|(i, _)| !used.contains(i)).map(|(_, (f, _))| *f).collect();
    assert!(
        bad.is_empty(),
        "files naming a metered host, not listed in ASKS with how their requests go through the proxy: {bad:?}"
    );
    assert!(stale.is_empty(), "ASKS lists files that name no metered host any more: {stale:?}");
}

#[test]
fn nothing_turns_the_proxy_off_but_what_is_listed() {
    let mut bad = Vec::new();
    let mut used = std::collections::BTreeSet::new();
    for (rel, text) in jobs_code() {
        for (n, line) in text.lines().enumerate() {
            let Some(m) = OFF.iter().find(|m| line.contains(*m)) else { continue };
            match BYPASS.iter().position(|(f, what, _)| *f == rel && line.contains(what)) {
                Some(i) => {
                    used.insert(i);
                }
                None => bad.push(format!("{rel}:{} ({m}): {}", n + 1, line.trim())),
            }
        }
    }
    let stale: Vec<_> = BYPASS
        .iter()
        .enumerate()
        .filter(|(i, _)| !used.contains(i))
        .map(|(_, (f, w, _))| format!("{f}: {w}"))
        .collect();
    assert!(bad.is_empty(), "lines turning the proxy off, not listed in BYPASS with why:\n{}", bad.join("\n"));
    assert!(stale.is_empty(), "BYPASS lists lines that aren't there any more: {stale:?}");
}
