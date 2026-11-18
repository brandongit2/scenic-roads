//! The checks' code digests (build/checks_code.rs, task #1467): a change to what a unit's module
//! compiles to moves its digest, and nothing else does.

use super::checks_code::{modules, Code};
use std::collections::BTreeMap;
use std::path::Path;

/// A folder of modules as `src/inputs/` holds them: `mod.rs` declaring `a` (with a child, a test
/// module and a compiled-in file) and `b`.
fn tree(d: &Path, edit: &[(&str, &str)]) -> BTreeMap<String, String> {
    let mut files: BTreeMap<&str, String> = [
        ("mod.rs", "pub mod a;\npub mod b;\n#[cfg(test)]\nmod tests;\n\npub fn shared() -> u32 {\n    1\n}\n"),
        (
            "a.rs",
            r#"//! Unit a.
mod child;

/// The themes the app knows.
pub const THEMES: [&str; 2] = ["terrain", "bathymetry"];

pub const SCHEMA: &str = include_str!("a/schema.json");

pub fn check(theme: &str, n: u32) -> Result<u32, String> {
    // (A theme it knows.)
    if !THEMES.contains(&theme) {
        return Err(format!("{theme} isn't a theme the app knows"));
    }
    match n {
        0 => Ok(child::least()),
        n => Ok(n),
    }
}

#[cfg(any(test, feature = "testkit"))]
pub const STAGED: [&str; 1] = ["x"];

#[test]
fn inline() {}

#[cfg(test)]
mod tests;
"#,
        ),
        ("a/child.rs", "pub fn least() -> u32 {\n    1\n}\n"),
        ("a/tests.rs", "#[test]\nfn t() {\n    assert_eq!(super::check(\"terrain\", 1), Ok(1));\n}\n"),
        ("a/schema.json", "{\"v\": 1}"),
        ("b.rs", "pub fn other() -> u32 {\n    2\n}\n"),
        ("tests.rs", "#[test]\nfn t() {}\n"),
    ]
    .into_iter()
    .map(|(p, t)| (p, t.to_string()))
    .collect();
    for (p, t) in edit {
        files.insert(p, t.to_string());
    }
    let _ = std::fs::remove_dir_all(d);
    for (p, t) in &files {
        let f = d.join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, t).unwrap();
    }
    modules(d).unwrap().into_iter().map(|(n, Code { digest, .. })| (n, digest)).collect()
}

/// `a.rs` with `from` replaced by `to`.
fn a_with(from: &str, to: &str) -> String {
    let a = tree_text("a.rs");
    assert!(a.contains(from), "{from}");
    a.replacen(from, to, 1)
}

fn tree_text(p: &str) -> String {
    let d = tempfile::tempdir().unwrap();
    tree(d.path(), &[]);
    std::fs::read_to_string(d.path().join(p)).unwrap()
}

#[test]
fn a_rule_table_its_code_or_a_file_it_compiles_in_moves_its_units_digest_alone() {
    let d = tempfile::tempdir().unwrap();
    let base = tree(d.path(), &[]);
    assert_eq!(base.keys().collect::<Vec<_>>(), ["a", "b"], "the modules mod.rs declares");
    assert!(base.values().all(|v| v.len() == 32));
    let themes =
        a_with("[&str; 2] = [\"terrain\", \"bathymetry\"]", "[&str; 3] = [\"terrain\", \"bathymetry\", \"buildings\"]");
    let message = a_with("isn't a theme the app knows", "isn't a theme");
    let child = "pub fn least() -> u32 {\n    0\n}\n";
    for (what, edit) in [
        ("its rule table", vec![("a.rs", themes.as_str())]),
        ("a finding's words", vec![("a.rs", message.as_str())]),
        ("a child module", vec![("a/child.rs", child)]),
        ("a file it compiles in", vec![("a/schema.json", "{\"v\": 2}")]),
    ] {
        let got = tree(d.path(), &edit);
        assert_ne!(got["a"], base["a"], "{what}");
        assert_eq!(got["b"], base["b"], "{what}: another unit's");
    }
}

#[test]
fn comments_layout_tests_and_other_modules_move_no_digest() {
    let d = tempfile::tempdir().unwrap();
    let base = tree(d.path(), &[]);
    let comment = a_with("    // (A theme it knows.)\n", "    // (A theme it knows, as the app does.)\n    // More.\n");
    let doc = a_with("/// The themes the app knows.", "/// The themes the app knows, by name.");
    let inner = a_with("//! Unit a.", "//! Unit a, its checks.");
    let layout = a_with(
        "pub const THEMES: [&str; 2] = [\"terrain\", \"bathymetry\"];",
        "pub const THEMES: [&str; 2] = [\n    \"terrain\",\n    \"bathymetry\",\n];",
    );
    let arm = a_with("0 => Ok(child::least()),", "0 => {\n            Ok(child::least())\n        }");
    let staged = a_with("[&str; 1] = [\"x\"]", "[&str; 2] = [\"x\", \"y\"]");
    let inline = a_with("fn inline() {}", "fn inline() {\n    assert!(true);\n}");
    for (what, edit) in [
        ("a comment", vec![("a.rs", comment.as_str())]),
        ("a doc comment", vec![("a.rs", doc.as_str())]),
        ("an inner doc comment", vec![("a.rs", inner.as_str())]),
        ("layout and a trailing comma", vec![("a.rs", layout.as_str())]),
        ("a match arm in braces", vec![("a.rs", arm.as_str())]),
        ("a testkit fixture", vec![("a.rs", staged.as_str())]),
        ("a #[test] function", vec![("a.rs", inline.as_str())]),
        ("its test module", vec![("a/tests.rs", "#[test]\nfn t2() {}\n")]),
        ("another module", vec![("b.rs", "pub fn other() -> u32 {\n    3\n}\n")]),
        ("mod.rs's own code", vec![("mod.rs", "pub mod a;\npub mod b;\n\npub fn shared() -> u32 {\n    2\n}\n")]),
    ] {
        let got = tree(d.path(), &edit);
        if what == "another module" {
            assert_ne!(got["b"], base["b"]);
        }
        assert_eq!(got["a"], base["a"], "{what}");
    }
}

/// A compiled-in file named other than by a string literal can't be digested: an error, never a
/// digest without it.
#[test]
fn an_include_by_anything_but_a_literal_is_an_error() {
    let d = tempfile::tempdir().unwrap();
    tree(d.path(), &[]);
    let by = format!("{}!(concat!(\"a/\", \"schema.json\"))", "include_str");
    let a = a_with("include_str!(\"a/schema.json\")", &by);
    std::fs::write(d.path().join("a.rs"), a).unwrap();
    let e = modules(d.path()).err().unwrap();
    assert!(e.contains("not a string literal"), "{e}");
}

/// The crate's own modules: every one digested, as build.rs does for the app.
#[test]
fn the_crates_modules_are_digested() {
    let m = modules(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src/inputs")).unwrap();
    let names: Vec<&str> = m.iter().map(|(n, _)| n.as_str()).collect();
    for unit in ["extents", "regions", "elevation", "gate", "gatetest"] {
        assert!(names.contains(&unit), "{unit}: {names:?}");
    }
    let regions = &m.iter().find(|(n, _)| n == "regions").unwrap().1;
    assert!(regions.files.iter().any(|f| f.ends_with("regions/resolve.rs")), "its children read");
    assert!(!regions.files.iter().any(|f| f.ends_with("regions/tests.rs")), "its tests not read");
}
