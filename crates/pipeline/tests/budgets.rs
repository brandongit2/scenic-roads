//! Every request a job makes to Wikimedia goes through its agent's proxy, taken from the pool's one
//! budget (docs/pool.md §7.7, §12 phase 4's seventh batch; crate::budget). The proxy and the sandbox
//! enforce it at egress whatever a step's code says (with the budgets on, a job has no other way
//! out); this lint, best-effort (a scan of the source's text) and run by tools/app/publish.sh, keeps
//! the steps honest besides:
//!
//! - every file of the jobs' code (the crates' Rust, dem/'s Python) that names a metered host is
//!   listed in `ASKS` with how its requests go (through the proxy, a client honouring
//!   `HTTPS_PROXY`), or that it asks nothing; and the Python that asks reports a 429 to the proxy
//!   (`budget.seen`, dem/budget.py), so the budget holds every process and Mac;
//! - nothing turns the proxy off (`.proxy(None)`, `ProxyHandler({})`, curl's `--noproxy`, a
//!   `no_proxy` of its own) but what `BYPASS` lists, with why;
//! - dem/budget.py's hosts are crate::budget's.
//!
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
    ("dem/budget.py", "the steps' side of the budget: its lists of the metered hosts, and the hold it tells the proxy"),
    ("dem/heritage.py", "urllib (honours HTTPS_PROXY); a 429 told (budget.seen)"),
    ("dem/heritage_eu.py", "urllib, curl (both honour HTTPS_PROXY); a 429 told (budget.seen)"),
    ("dem/heritagewd.py", "curl (honours HTTPS_PROXY); a 429 told (budget.seen)"),
    ("dem/items.py", "names Wikidata's entities in its queries, asked through heritagewd.sparql"),
    ("dem/areadetails.py", "names Wikidata's entities in its queries, asked through heritagewd.sparql"),
    ("dem/federal.py", "names an item's page as a link, asks nothing"),
    ("dem/pageviews.py", "curl and urllib (both honour HTTPS_PROXY): a month's dump a connection; a 429 fails the month (curl --fail), tried again by the step's next run, not told"),
];

/// The Python that asks, so reports a 429 (`budget.seen`).
const REPORTS: &[&str] = &["dem/heritage.py", "dem/heritage_eu.py", "dem/heritagewd.py"];

/// What turns the proxy off, in a line.
const OFF: &[&str] = &[".proxy(None)", "ProxyHandler({})", "--noproxy", "no_proxy", "NO_PROXY"];

/// The lines turning the proxy off: by file and what the line says, with why.
const BYPASS: &[(&str, &str, &str)] = &[
    ("crates/pipeline/src/bin/scenic-metrics.rs", ".proxy(None)", "the owner's metrics tool, run by hand on this Mac, asking the agents here and on the LAN: never a job"),
    ("dem/budget.py", "ProxyHandler({})", "the hold told to the proxy itself, on this Mac"),
    ("crates/pipeline/src/budget/proxy.rs", "NO_PROXY", "the job's own Mac (its coordinator on 127.0.0.1) direct: the sandbox allows it"),
    ("crates/pipeline/src/agent/mod.rs", "NO_PROXY=localhost,127.0.0.1,::1", "a test: the job's environment as the proxy gives it"),
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

fn jobs_code() -> Vec<(String, String)> {
    let root = root();
    let mut fs = Vec::new();
    files(&root.join("crates"), "rs", &mut fs);
    files(&root.join("dem"), "py", &mut fs);
    let mut out: Vec<(String, String)> = fs
        .into_iter()
        .map(|f| (f.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"), std::fs::read_to_string(&f).unwrap_or_default()))
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
        if !names_metered(&text) {
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
    assert!(bad.is_empty(), "files naming a metered host, not listed in ASKS with how their requests go through the proxy: {bad:?}");
    assert!(stale.is_empty(), "ASKS lists files that name no metered host any more: {stale:?}");
}

#[test]
fn the_python_that_asks_tells_a_429() {
    let code: std::collections::BTreeMap<String, String> = jobs_code().into_iter().collect();
    for f in REPORTS {
        let t = &code[*f];
        assert!(t.contains("import budget") && t.contains("budget.seen("), "{f}: asks a metered host, so tells the proxy a 429 (budget.seen)");
    }
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
    let stale: Vec<_> = BYPASS.iter().enumerate().filter(|(i, _)| !used.contains(i)).map(|(_, (f, w, _))| format!("{f}: {w}")).collect();
    assert!(bad.is_empty(), "lines turning the proxy off, not listed in BYPASS with why:\n{}", bad.join("\n"));
    assert!(stale.is_empty(), "BYPASS lists lines that aren't there any more: {stale:?}");
}

/// dem/budget.py's metered hosts are crate::budget's (the Python names them to tell the proxy only
/// a metered host's 429).
#[test]
fn the_pythons_hosts_are_the_budgets() {
    let t = std::fs::read_to_string(root().join("dem/budget.py")).unwrap();
    let list = |name: &str| -> Vec<String> {
        let start = t.find(&format!("\n{name} = (")).unwrap_or_else(|| panic!("{name} in dem/budget.py")) + name.len() + 5;
        let end = start + t[start..].find(')').unwrap();
        t[start..end].split(',').map(|s| s.trim().trim_matches('"').to_string()).filter(|s| !s.is_empty()).collect()
    };
    assert_eq!(list("QUERY"), QUERY_HOSTS);
    assert_eq!(list("QUERY_DOMAINS"), QUERY_DOMAINS);
    assert_eq!(list("WIKIMEDIA"), WIKIMEDIA);
}
