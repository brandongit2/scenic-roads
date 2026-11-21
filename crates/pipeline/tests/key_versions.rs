//! Task #1253: a step whose key is made outside the steps table (it declares no `versions`) has its
//! cost version derived from the `key_versions` its row declares (pipeline::agent::steps). Every
//! string in the crates' code (tests left out) naming such a step and then a version constant (a
//! `…_V` or `…VERSION`: `"<step> {V}"`, `"<step> {}"` with V its first argument, or `"<step> "` with
//! V the next expression, as `["<step> ", &V.to_string()].concat()`) is of a row declaring that
//! constant in its `key_versions`, and every row declaring `key_versions` has such a string. Found
//! from the code, so a key maker's version its cost version leaves out fails here.
//!
//! scans: crates

mod common;

use common::*;
use pipeline::agent::steps::TABLE;
use std::collections::{BTreeMap, BTreeSet};

/// Whether `c` (a path's last segment) names a version constant.
fn versioned(c: &str) -> bool {
    let caps = !c.is_empty() && c.chars().all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_');
    caps && (c.ends_with("_V") || c.ends_with("VERSION"))
}

/// The last segment of the path that `s` starts with, past spaces, commas and `&`.
fn path_at(s: &str) -> &str {
    let s = s.trim_start_matches([' ', ',', '\n', '&']);
    let end = s.find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == ':')).unwrap_or(s.len());
    s[..end].rsplit("::").next().unwrap_or("")
}

/// The version constants each of `names` is named with in `src` (code, tests left out).
fn named_versions(src: &str, names: &[&str], out: &mut BTreeMap<String, BTreeSet<String>>) {
    for name in names {
        let pat = format!("\"{name} ");
        for (at, _) in src.match_indices(&pat) {
            let rest = &src[at + pat.len()..];
            let Some(close) = rest.find('"') else { continue };
            let (lit, after) = (&rest[..close], &rest[close + 1..]);
            let c = if let Some(b) = lit.strip_prefix('{').and_then(|l| l.split_once('}')).map(|(b, _)| b) {
                if b.is_empty() {
                    path_at(after)
                } else {
                    b.rsplit("::").next().unwrap_or("")
                }
            } else if lit.is_empty() {
                path_at(after)
            } else {
                continue;
            };
            if versioned(c) {
                out.entry(name.to_string()).or_default().insert(c.to_string());
            }
        }
    }
}

#[test]
fn every_key_made_outside_the_table_declares_its_versions() {
    let root = root();
    let names: Vec<&str> = TABLE.iter().filter(|r| r.versions.is_empty()).map(|r| r.name).collect();
    let mut rs = Vec::new();
    files(&root.join("crates"), "rs", &mut rs);
    let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for p in &rs {
        let text = std::fs::read_to_string(p).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let code: Vec<String> = lines.iter().map(|l| code_of(l)).collect();
        let test = test_lines(&code);
        let src: Vec<&str> = lines.iter().zip(&test).filter(|(_, t)| !**t).map(|(l, _)| *l).collect();
        named_versions(&src.join("\n"), &names, &mut found);
    }
    let declaring: BTreeSet<&str> = TABLE.iter().filter(|r| !r.key_versions.is_empty()).map(|r| r.name).collect();
    assert_eq!(found.keys().map(String::as_str).collect::<BTreeSet<_>>(), declaring, "{found:?}");
    let steps = std::fs::read_to_string(root.join("crates/pipeline/src/agent/steps.rs")).unwrap();
    for (name, consts) in &found {
        let at = steps.find(&format!("key_versions: &[(\"{name}\", ||")).unwrap_or_else(|| panic!("{name}"));
        let line = steps[at..].lines().next().unwrap();
        for c in consts {
            assert!(line.contains(&format!("{c}.to_string()")), "{name}: its key's {c}, declared {line}");
        }
    }
}

/// The scan reads each way a key names a version.
#[test]
fn the_scan_reads_each_way_a_key_names_a_version() {
    let src = r#"
        let a = format!("lakefill {LAKEFILL_V}");
        let b = format!("names-todo {}", landmarks::namestodo::VERSION);
        let c = ["pack ", &PACK_V.to_string()].concat();
        let d = format!("lo {date}");
        let e = format!("prune {} left", n);
        let f = "gc format version {MIN_VERSION}";
    "#;
    let mut found = BTreeMap::new();
    named_versions(src, &["lakefill", "names-todo", "pack", "lo", "prune", "gc"], &mut found);
    let got: Vec<(&str, Vec<&str>)> =
        found.iter().map(|(k, v)| (k.as_str(), v.iter().map(String::as_str).collect())).collect();
    assert_eq!(got, [("lakefill", vec!["LAKEFILL_V"]), ("names-todo", vec!["VERSION"]), ("pack", vec!["PACK_V"])]);
}
