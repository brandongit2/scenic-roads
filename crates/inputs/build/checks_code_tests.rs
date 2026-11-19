//! The checks' code digests (build/checks_code.rs, task #1467): a change to the code a unit's
//! checks compile to, in its module or anything it names (across modules and crates), moves its
//! digest, and nothing else does; the typed versions left are held to the code they cover.

use super::checks_code::{codes, World};
use std::collections::BTreeMap;
use std::path::Path;

/// A workspace as the inputs crate's: `inputs` (its gate's `mod.rs` with a registry naming the units
/// `a` and `b`, `gate`, and `other`, a module outside `src/inputs/`) and a path dependency `dep`.
fn base() -> BTreeMap<&'static str, String> {
    [
        ("inputs/Cargo.toml", "[package]\nname = \"inputs\"\n\n[dependencies]\ndep = { path = \"../dep\" }\nserde = \"1\"\n\n[dev-dependencies]\nnot_read = { path = \"../nowhere\" }\n"),
        ("inputs/src/lib.rs", "pub mod inputs;\npub mod other;\n"),
        ("inputs/src/other.rs", "pub fn helper() -> u32 {\n    1\n}\n\npub fn unused() -> u32 {\n    1\n}\n"),
        (
            "inputs/src/inputs/mod.rs",
            "pub mod a;\npub mod b;\npub mod gate;\n#[cfg(test)]\nmod tests;\n\npub struct Finding;\n\nimpl Finding {\n    pub fn holds(&self) -> bool {\n        true\n    }\n}\n\npub const UNITS: [&str; 2] = [a::UNIT, b::UNIT];\n",
        ),
        (
            "inputs/src/inputs/a.rs",
            r#"//! Unit a.
mod child;

use super::Finding;

pub const UNIT: &str = "a";

/// The themes the app knows.
pub const THEMES: [&str; 2] = ["terrain", "bathymetry"];

pub const SCHEMA: &str = include_str!("a/schema.json");

pub fn check(theme: &str, n: u32) -> Result<u32, String> {
    // (A theme it knows.)
    if !THEMES.contains(&theme) || !Finding.holds() {
        return Err(format!("{theme} isn't a theme the app knows"));
    }
    let pair = (n);
    match pair {
        0 => Ok(child::least() + crate::other::helper() + dep::geo::far()),
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
        ("inputs/src/inputs/a/child.rs", "pub fn least() -> u32 {\n    1\n}\n"),
        ("inputs/src/inputs/a/tests.rs", "#[test]\nfn t() {\n    assert_eq!(super::check(\"terrain\", 1), Ok(1));\n}\n"),
        ("inputs/src/inputs/a/schema.json", "{\"v\": 1}"),
        ("inputs/src/inputs/b.rs", "pub const UNIT: &str = \"b\";\n\npub fn other() -> u32 {\n    2\n}\n"),
        ("inputs/src/inputs/gate.rs", "pub fn decide() -> bool {\n    super::Finding.holds()\n}\n"),
        ("inputs/src/inputs/tests.rs", "#[test]\nfn t() {}\n"),
        ("dep/Cargo.toml", "[package]\nname = \"dep\"\n"),
        ("dep/src/lib.rs", "pub mod geo;\npub mod unused;\n"),
        ("dep/src/geo.rs", "pub fn far() -> u32 {\n    3\n}\n"),
        ("dep/src/unused.rs", "pub fn nothing() -> u32 {\n    4\n}\n"),
    ]
    .into_iter()
    .map(|(p, t)| (p, t.to_string()))
    .collect()
}

/// The workspace with `edit` (path, from, to) applied, in `d`: each unit's code.
fn codes_with(d: &Path, edit: &[(&str, &str, &str)]) -> BTreeMap<String, String> {
    let mut files = base();
    for (p, from, to) in edit {
        let t = files.get_mut(p).unwrap();
        assert!(t.contains(from), "{p}: {from}");
        *t = t.replacen(from, to, 1);
    }
    let _ = std::fs::remove_dir_all(d);
    for (p, t) in &files {
        let f = d.join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, t).unwrap();
    }
    codes(&World::read(&d.join("inputs")).unwrap()).into_iter().collect()
}

#[test]
fn what_a_unit_names_moves_its_digest_and_nothing_else_does() {
    let d = tempfile::tempdir().unwrap();
    let base = codes_with(d.path(), &[]);
    assert_eq!(base.keys().collect::<Vec<_>>(), ["a", "b", "gate"], "the modules mod.rs declares");
    let a = "inputs/src/inputs/a.rs";
    let mod_rs = "inputs/src/inputs/mod.rs";
    // (Each: what it edits, and the units whose code it moves.)
    let cases: [(&str, Vec<(&str, &str, &str)>, &[&str]); 9] = [
        (
            "its rule table",
            vec![(
                a,
                "[&str; 2] = [\"terrain\", \"bathymetry\"]",
                "[&str; 3] = [\"terrain\", \"bathymetry\", \"buildings\"]",
            )],
            &["a"],
        ),
        ("a finding's words", vec![(a, "isn't a theme the app knows", "isn't a theme")], &["a"]),
        ("a child module", vec![("inputs/src/inputs/a/child.rs", "    1\n", "    0\n")], &["a"]),
        ("a file it compiles in", vec![("inputs/src/inputs/a/schema.json", "1", "2")], &["a"]),
        ("a tuple for a value in parentheses", vec![(a, "let pair = (n);", "let pair = (n,);")], &["a"]),
        (
            "a function of another module it calls",
            vec![("inputs/src/other.rs", "    1\n}\n\npub fn unused", "    2\n}\n\npub fn unused")],
            &["a"],
        ),
        ("a function of a crate it depends on", vec![("dep/src/geo.rs", "3", "5")], &["a"]),
        ("the gate's own item it names", vec![(mod_rs, "        true\n", "        false\n")], &["a", "gate"]),
        ("another unit", vec![("inputs/src/inputs/b.rs", "    2\n", "    7\n")], &["b"]),
    ];
    for (what, edit, moved) in cases {
        let got = codes_with(d.path(), &edit);
        for (unit, code) in &got {
            assert_eq!(code != &base[unit], moved.contains(&unit.as_str()), "{what}: {unit}'s code");
        }
    }
}

#[test]
fn comments_layout_tests_and_what_no_unit_names_move_no_digest() {
    let d = tempfile::tempdir().unwrap();
    let base = codes_with(d.path(), &[]);
    let a = "inputs/src/inputs/a.rs";
    let cases: [(&str, Vec<(&str, &str, &str)>); 12] = [
        (
            "a comment",
            vec![(a, "    // (A theme it knows.)\n", "    // (A theme it knows, as the app does.)\n    // More.\n")],
        ),
        ("a doc comment", vec![(a, "/// The themes the app knows.", "/// The themes the app knows, by name.")]),
        ("an inner doc comment", vec![(a, "//! Unit a.", "//! Unit a, its checks.")]),
        (
            "layout and a trailing comma",
            vec![(
                a,
                "[&str; 2] = [\"terrain\", \"bathymetry\"];",
                "[&str; 2] = [\n    \"terrain\",\n    \"bathymetry\",\n];",
            )],
        ),
        (
            "a match arm in braces",
            vec![(
                a,
                "0 => Ok(child::least() + crate::other::helper() + dep::geo::far()),",
                "0 => {\n            Ok(child::least() + crate::other::helper() + dep::geo::far())\n        }",
            )],
        ),
        ("a testkit fixture", vec![(a, "[&str; 1] = [\"x\"]", "[&str; 2] = [\"x\", \"y\"]")]),
        ("a #[test] function", vec![(a, "fn inline() {}", "fn inline() {\n    assert!(true);\n}")]),
        ("its test module", vec![("inputs/src/inputs/a/tests.rs", "fn t()", "fn t2()")]),
        ("the gate's test module", vec![("inputs/src/inputs/tests.rs", "fn t()", "fn t2()")]),
        (
            "a function no unit calls",
            vec![("inputs/src/other.rs", "pub fn unused() -> u32 {\n    1", "pub fn unused() -> u32 {\n    9")],
        ),
        ("a module of a dependency no unit names", vec![("dep/src/unused.rs", "4", "8")]),
        ("a dev-dependency's manifest line", vec![("inputs/Cargo.toml", "not_read", "not_read_either")]),
    ];
    for (what, edit) in cases {
        assert_eq!(codes_with(d.path(), &edit), base, "{what}");
    }
    // The registry naming a unit reaches none of its code: b changed, the gate's is as it was.
    let got = codes_with(d.path(), &[("inputs/src/inputs/b.rs", "    2\n", "    7\n")]);
    assert_eq!(got["gate"], base["gate"]);
}

/// A compiled-in file named other than by a string literal can't be digested: an error, never a
/// digest without it; a file that doesn't parse is an error naming where.
#[test]
fn an_include_by_anything_but_a_literal_or_a_syntax_error_is_an_error() {
    let d = tempfile::tempdir().unwrap();
    codes_with(d.path(), &[]);
    let a = d.path().join("inputs/src/inputs/a.rs");
    let text = std::fs::read_to_string(&a).unwrap();
    let by = format!("{}!(concat!(\"a/\", \"schema.json\"))", "include_str");
    std::fs::write(&a, text.replacen(r#"include_str!("a/schema.json")"#, &by, 1)).unwrap();
    let e = World::read(&d.path().join("inputs")).err().unwrap();
    assert!(e.contains("not a string literal"), "{e}");
    std::fs::write(&a, text.replacen("pub fn check(", "pub fn check((", 1)).unwrap();
    let e = World::read(&d.path().join("inputs")).err().unwrap();
    assert!(e.contains("a.rs:13:"), "{e}");
}

/// What build.rs compiled in is what the sources say now (a stale OUT_DIR, or a file read and not
/// rerun on, would differ).
#[test]
fn the_codes_compiled_in_are_the_sources_now() {
    let w = World::read(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
    for (module, code) in codes(&w) {
        assert_eq!(crate::inputs::code_of(&format!("inputs::inputs::{module}")), code, "{module}");
    }
}

/// The typed versions left are held to the code they cover: a change there with the number as it
/// was fails here, saying what to do.
#[test]
fn the_resolution_and_the_measure_rules_are_held_to_their_code() {
    let w = World::read(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
    let now = resolution_code(&w);
    assert_eq!(
        now,
        crate::inputs::regions::RESOLUTION_CODE,
        "the regions' resolution code changed: bump regions::RESOLUTION if what an unchanged region resolves to \
         (or its problems' words) changes, and set regions::RESOLUTION_CODE to \"{now}\" either way"
    );
    for (step, now) in rule_codes(&w) {
        let stored = crate::inputs::elevation::RULE_CODES.iter().find(|(s, _)| *s == step).map(|(_, d)| *d);
        assert_eq!(
            Some(now.as_str()),
            stored,
            "elevation's {step} step's measuring code changed: raise its Step::rule if what it measures changes, \
             and set its digest in elevation::RULE_CODES to \"{now}\" either way"
        );
    }
}

/// The code the regions' `RESOLUTION` covers: the items of their resolution's modules (`resolve`,
/// `tiles`, `geom`, `land`, `stored`, `shapes`, `data`) and all they name. Held to
/// `regions::RESOLUTION_CODE` by a test.
pub fn resolution_code(w: &World) -> String {
    let modules = ["resolve", "tiles", "geom", "land", "stored", "shapes", "data"];
    let path = |m: &str| vec!["inputs".to_string(), "regions".to_string(), m.to_string()];
    w.digest(modules.iter().flat_map(|m| w.under("inputs", &path(m))).collect())
}

/// The code each elevation measure step's rule (`Step::rule`) covers, by the step's name: its
/// measuring code and all it names (the headers' `measure_now`, which runs every step;
/// `presence_of`; `hollow_of`). Held to `elevation::RULE_CODES` by a test.
pub fn rule_codes(w: &World) -> Vec<(&'static str, String)> {
    let m = vec!["inputs".to_string(), "elevation".to_string()];
    [("headers", "measure_now"), ("presence", "presence_of"), ("hollow", "hollow_of")]
        .into_iter()
        .map(|(step, root)| (step, w.digest(w.named("inputs", &m, &[root]))))
        .collect()
}
