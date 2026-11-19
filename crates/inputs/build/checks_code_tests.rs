//! The checks' code digests (build/checks_code.rs, task #1467): a change to the code a unit's
//! checks compile to, in its module or anything it names (across modules and crates), moves its
//! digest, and nothing else does; the typed versions left are held to the code they cover.
#![cfg(test)]

use super::checks_code::{codes, Id, Kind, World};
use std::collections::BTreeMap;
use std::path::Path;

/// A workspace as the inputs crate's: `inputs` (its gate's `mod.rs` with a registry naming the units
/// `a` and `b`, `gate`; `other` and `extra`, modules outside `src/inputs/`), a path dependency
/// `dep`, and a crate for each form a manifest names one by (`wsdep` the workspace's, `tabled` by a
/// table of its own, `unixdep` a target's, `real` renamed `renamed`).
fn base() -> BTreeMap<&'static str, String> {
    let f = "pub fn f() -> u32 {\n    1\n}\n";
    [
        ("Cargo.toml", "[workspace]\nmembers = [\"inputs\", \"dep\"]\n\n[workspace.dependencies]\nwsdep = { path = \"wsdep\" }\n".into()),
        (
            "inputs/Cargo.toml",
            "[package]\nname = \"inputs\"\n\n[dependencies]\ndep = { path = \"../dep\" }\nserde = \"1\"\nwsdep.workspace = true\nrenamed = { path = \"../real\", package = \"real\" } # (Its name in code.)\n\n[target.'cfg(unix)'.dependencies]\nunixdep = { path = \"../unixdep\" }\n\n[dependencies.tabled]\npath = \"../tabled\"\nfeatures = [\n    \"x\",\n]\n\n[dev-dependencies]\nnot_read = { path = \"../nowhere\" }\n".into(),
        ),
        ("inputs/src/lib.rs", "pub mod inputs;\npub mod other;\npub mod extra;\n".into()),
        (
            "inputs/src/other.rs",
            "pub fn helper() -> u32 {\n    1\n}\n\npub fn unused() -> u32 {\n    1\n}\n\npub fn deep() -> u32 {\n    5\n}\n\npub fn one() -> u32 {\n    1\n}\n\npub enum Mode {\n    Fast,\n    Slow,\n}\n\npub struct Tool;\n\nimpl Tool {\n    pub fn used(&self) -> u32 {\n        1\n    }\n\n    pub fn idle(&self) -> u32 {\n        1\n    }\n}\n\nimpl std::fmt::Display for Tool {\n    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {\n        write!(f, \"tool\")\n    }\n}\n".into(),
        ),
        ("inputs/src/extra.rs", "impl crate::other::Tool {\n    pub fn elsewhere(&self) -> u32 {\n        4\n    }\n}\n".into()),
        (
            "inputs/src/inputs/mod.rs",
            "pub mod a;\npub mod b;\npub mod gate;\n#[cfg(test)]\nmod tests;\n\npub struct Finding;\n\nimpl Finding {\n    pub fn holds(&self) -> bool {\n        true\n    }\n}\n\npub const UNITS: [&str; 2] = [a::UNIT, b::UNIT];\n".into(),
        ),
        (
            "inputs/src/inputs/a.rs",
            r#"//! Unit a.
mod child;

use super::Finding;
use crate::other::Mode::*;
use dep::twice::Twice;

pub const UNIT: &str = "a";

/// The themes the app knows.
pub const THEMES: [&str; 2] = ["terrain", "bathymetry"];

pub const SCHEMA: &str = include_str!("a/schema.json");

pub const LIMIT: u32 = 9;

#[derive(serde::Deserialize)]
pub struct Opts {
    #[serde(default = "crate::other::one")]
    pub n: u32,
}

pub fn check(theme: &str, n: u32) -> Result<u32, String> {
    // (A theme it knows.)
    if !THEMES.contains(&theme) || !Finding.holds() {
        return Err(format!("{theme} isn't a theme the app knows, past {LIMIT}"));
    }
    let pair = (n);
    match pair {
        0 => Ok(child::least() + crate::other::helper() + dep::geo::far()),
        1 => Ok(::dep::geo::near() + n.twice()),
        2 => Ok({
            use crate::other::*;
            deep()
        }),
        3 => Ok(crate::other::Tool.used() + crate::other::Tool.elsewhere() + Fast as u32),
        n => Ok(n),
    }
}

#[cfg(any(test, feature = "testkit"))]
pub const STAGED: [&str; 1] = ["x"];

#[test]
fn inline() {}

#[cfg(test)]
mod tests;
"#
            .into(),
        ),
        ("inputs/src/inputs/a/child.rs", "pub fn least() -> u32 {\n    1\n}\n".into()),
        ("inputs/src/inputs/a/tests.rs", "#[test]\nfn t() {\n    assert_eq!(super::check(\"terrain\", 1), Ok(1));\n}\n".into()),
        ("inputs/src/inputs/a/schema.json", "{\"v\": 1}".into()),
        (
            "inputs/src/inputs/b.rs",
            "pub const UNIT: &str = \"b\";\n\npub fn other() -> u32 {\n    let dep = 2;\n    dep + wsdep::f() + renamed::f() + unixdep::f() + tabled::f()\n}\n".into(),
        ),
        ("inputs/src/inputs/gate.rs", "pub fn decide() -> bool {\n    super::Finding.holds()\n}\n".into()),
        ("inputs/src/inputs/tests.rs", "#[test]\nfn t() {}\n".into()),
        ("dep/Cargo.toml", "[package]\nname = \"dep\"\n".into()),
        ("dep/src/lib.rs", "pub mod geo;\npub mod twice;\npub mod unused;\n".into()),
        ("dep/src/geo.rs", "pub fn far() -> u32 {\n    3\n}\n\npub fn near() -> u32 {\n    6\n}\n".into()),
        (
            "dep/src/twice.rs",
            "pub trait Twice {\n    fn twice(self) -> Self;\n}\n\nmacro_rules! twice {\n    ($t:ty, $k:expr) => {\n        impl Twice for $t {\n            fn twice(self) -> $t {\n                self * $k\n            }\n        }\n    };\n}\n\ntwice!(u32, 2);\n".into(),
        ),
        ("dep/src/unused.rs", "pub fn nothing() -> u32 {\n    4\n}\n".into()),
        ("wsdep/Cargo.toml", "[package]\nname = \"wsdep\"\n".into()),
        ("wsdep/src/lib.rs", f.into()),
        ("tabled/Cargo.toml", "[package]\nname = \"tabled\"\n".into()),
        ("tabled/src/lib.rs", f.into()),
        ("unixdep/Cargo.toml", "[package]\nname = \"unixdep\"\n".into()),
        ("unixdep/src/lib.rs", f.into()),
        ("real/Cargo.toml", "[package]\nname = \"real\"\n".into()),
        ("real/src/lib.rs", f.into()),
    ]
    .into_iter()
    .collect()
}

/// The workspace with `edit` (path, from, to) applied, written to `d`, read.
fn read_with(d: &Path, edit: &[(&str, &str, &str)]) -> Result<World, String> {
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
    World::read(&d.join("inputs"))
}

/// Each unit's code, the workspace with `edit` applied.
fn codes_with(d: &Path, edit: &[(&str, &str, &str)]) -> BTreeMap<String, String> {
    codes(&read_with(d, edit).unwrap()).unwrap().into_iter().collect()
}

#[test]
fn what_a_unit_names_moves_its_digest_and_nothing_else_does() {
    let d = tempfile::tempdir().unwrap();
    let base = codes_with(d.path(), &[]);
    assert_eq!(base.keys().collect::<Vec<_>>(), ["a", "b", "gate"], "the modules mod.rs declares");
    let (a, b, other) = ("inputs/src/inputs/a.rs", "inputs/src/inputs/b.rs", "inputs/src/other.rs");
    let mod_rs = "inputs/src/inputs/mod.rs";
    // (Each: what it edits, and the units whose code it moves.)
    let cases: Vec<(&str, Vec<(&str, &str, &str)>, &[&str])> = vec![
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
        ("a const named only in a format string", vec![(a, "LIMIT: u32 = 9", "LIMIT: u32 = 10")], &["a"]),
        ("a child module", vec![("inputs/src/inputs/a/child.rs", "    1\n", "    0\n")], &["a"]),
        ("a file it compiles in", vec![("inputs/src/inputs/a/schema.json", "1", "2")], &["a"]),
        ("a tuple for a value in parentheses", vec![(a, "let pair = (n);", "let pair = (n,);")], &["a"]),
        (
            "a function of another module it calls",
            vec![(other, "    1\n}\n\npub fn unused", "    2\n}\n\npub fn unused")],
            &["a"],
        ),
        (
            "a function an attribute's string names",
            vec![(other, "pub fn one() -> u32 {\n    1", "pub fn one() -> u32 {\n    2")],
            &["a"],
        ),
        ("a function a function's own glob brings in", vec![(other, "    5\n", "    6\n")], &["a"]),
        ("an enum whose variants a glob brings in", vec![(other, "    Slow,\n", "    Slow,\n    Still,\n")], &["a"]),
        (
            "a method it calls",
            vec![(other, "pub fn used(&self) -> u32 {\n        1", "pub fn used(&self) -> u32 {\n        2")],
            &["a"],
        ),
        ("a method of its type's impl in another module", vec![("inputs/src/extra.rs", "4", "5")], &["a"]),
        ("a crates.io trait's impl for its type", vec![(other, "\"tool\"", "\"tools\"")], &["a"]),
        ("a function of a crate it depends on", vec![("dep/src/geo.rs", "3", "5")], &["a"]),
        ("a function it names by a leading ::", vec![("dep/src/geo.rs", "6", "7")], &["a"]),
        ("a dependency's macro making the impl it calls", vec![("dep/src/twice.rs", "self * $k", "self + $k")], &["a"]),
        ("that macro's invocation", vec![("dep/src/twice.rs", "twice!(u32, 2)", "twice!(u32, 3)")], &["a"]),
        ("the gate's own item it names", vec![(mod_rs, "        true\n", "        false\n")], &["a", "gate"]),
        ("another unit", vec![(b, "let dep = 2;", "let dep = 7;")], &["b"]),
        ("a workspace dependency", vec![("wsdep/src/lib.rs", "1", "2")], &["b"]),
        ("a renamed dependency", vec![("real/src/lib.rs", "1", "2")], &["b"]),
        ("a target's dependency", vec![("unixdep/src/lib.rs", "1", "2")], &["b"]),
        ("a dependency by a table of its own", vec![("tabled/src/lib.rs", "1", "2")], &["b"]),
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
    let (a, other) = ("inputs/src/inputs/a.rs", "inputs/src/other.rs");
    let cases: Vec<(&str, Vec<(&str, &str, &str)>)> = vec![
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
            vec![(other, "pub fn unused() -> u32 {\n    1", "pub fn unused() -> u32 {\n    9")],
        ),
        (
            "a method of a type it reaches that nothing calls",
            vec![(other, "pub fn idle(&self) -> u32 {\n        1", "pub fn idle(&self) -> u32 {\n        2")],
        ),
        ("a module of a dependency no unit names", vec![("dep/src/unused.rs", "4", "8")]),
        ("a dev-dependency's manifest line", vec![("inputs/Cargo.toml", "not_read", "not_read_either")]),
        ("a manifest's comment", vec![("inputs/Cargo.toml", "# (Its name in code.)", "# (Its name.)")]),
    ];
    for (what, edit) in cases {
        assert_eq!(codes_with(d.path(), &edit), base, "{what}");
    }
    // The registry naming a unit reaches none of its code: b changed, the gate's is as it was.
    let got = codes_with(d.path(), &[("inputs/src/inputs/b.rs", "let dep = 2;", "let dep = 7;")]);
    assert_eq!(got["gate"], base["gate"]);
}

/// A compiled-in file named other than by a string literal can't be digested: an error, never a
/// digest without it; a file that doesn't parse is an error naming where; so is a manifest's
/// dependency of a form not read, and an item of `mod.rs` but the registry's naming a unit.
#[test]
fn what_it_cant_read_or_cut_is_an_error() {
    let d = tempfile::tempdir().unwrap();
    let a = "inputs/src/inputs/a.rs";
    let toml = "inputs/Cargo.toml";
    let by = format!("{}!(concat!(\"a/\", \"schema.json\"))", "include_str");
    let cases: [(&str, (&str, &str, &str), &str); 5] = [
        ("an include by a macro", (a, r#"include_str!("a/schema.json")"#, &by), "not a string literal"),
        ("a syntax error", (a, "pub fn check(", "pub fn check(("), "a.rs:23:"),
        ("a dependency's value", (toml, "serde = \"1\"", "serde = 1"), "Cargo.toml:6: a dependency this doesn't read"),
        (
            "a dependency's key",
            (toml, "path = \"../dep\" }", "path = \"../dep\", from = \"x\" }"),
            "Cargo.toml:5: a dependency key",
        ),
        ("a dependency table's key", (toml, "features = [", "frozen = ["), "Cargo.toml:15: a dependency key"),
    ];
    for (what, edit, says) in cases {
        let e = read_with(d.path(), &[edit]).err().unwrap_or_else(|| panic!("{what}: read"));
        assert!(e.contains(says), "{what}: {e}");
    }
    let named = (
        "inputs/src/inputs/mod.rs",
        "pub const UNITS",
        "pub fn first() -> &'static str {\n    a::UNIT\n}\n\npub const UNITS",
    );
    let e = codes(&read_with(d.path(), &[named]).unwrap()).err().unwrap();
    assert!(e.contains("`first` names inputs::a"), "{e}");
}

/// What build.rs compiled in is what the sources say now (a stale OUT_DIR, or a file read and not
/// rerun on, would differ).
#[test]
fn the_codes_compiled_in_are_the_sources_now() {
    let w = World::read(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
    for (module, code) in codes(&w).unwrap() {
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

fn path(p: &[&str]) -> Vec<String> {
    p.iter().map(|s| s.to_string()).collect()
}

/// The method `name` of the impls in module `m` of the inputs crate of a type named `ty`.
fn method(w: &World, m: &[String], ty: &str, name: &str) -> Vec<Id> {
    let items = &w.crates["inputs"].modules[m].items;
    let of_ty =
        |h: usize| matches!(&items[h].kind, Kind::Impl { ty: Some(p), .. } if p.last().is_some_and(|l| l == ty));
    let v: Vec<Id> = (0..items.len())
        .filter(|&i| matches!(&items[i].kind, Kind::Method(h, n) if n == name && of_ty(*h)))
        .map(|i| ("inputs".to_string(), m.to_vec(), i))
        .collect();
    assert!(!v.is_empty(), "{}: {ty}::{name}", m.join("::"));
    v
}

/// The code the regions' `RESOLUTION` covers: the items of their resolution's modules (`resolve`,
/// `tiles`, `geom`, `land`, `stored`, `shapes`, `data`), what of `regions/mod.rs` shapes a
/// resolution (a resolution's making and its key, `Regions::made`, `outcome_key`; its budgets,
/// `max_mb`, `op_budget_s`; its data, `data_from`, `Prepared::land_open`; a stored one taken,
/// `accepted_resolution`, `Recorded`), and all they name. Held to `regions::RESOLUTION_CODE` by a
/// test.
fn resolution_code(w: &World) -> String {
    let regions = path(&["inputs", "regions"]);
    let mut start: Vec<Id> = ["resolve", "tiles", "geom", "land", "stored", "shapes", "data"]
        .iter()
        .flat_map(|m| w.under("inputs", &[regions.clone(), vec![m.to_string()]].concat()))
        .collect();
    for (ty, m) in [
        ("Regions", "made"),
        ("Regions", "outcome_key"),
        ("Regions", "accepted_resolution"),
        ("Regions", "data_from"),
        ("Prepared", "land_open"),
    ] {
        start.extend(method(w, &regions, ty, m));
    }
    let named = w.named("inputs", &regions, &["max_mb", "op_budget_s", "Recorded"]);
    assert_eq!(named.len(), 3, "regions/mod.rs: max_mb, op_budget_s, Recorded");
    start.extend(named);
    w.digest(start, &[])
}

/// The code each elevation measure step's rule (`Step::rule`) covers, by the step's name: its
/// measuring code and all it names (`headers_of`, `presence_of`, `hollow_of`), `Step::rule` itself
/// left out. Held to `elevation::RULE_CODES` by a test.
fn rule_codes(w: &World) -> Vec<(&'static str, String)> {
    let m = path(&["inputs", "elevation"]);
    let rule = method(w, &m, "Step", "rule");
    [("headers", "headers_of"), ("presence", "presence_of"), ("hollow", "hollow_of")]
        .into_iter()
        .map(|(step, root)| {
            let start = w.named("inputs", &m, &[root]);
            assert_eq!(start.len(), 1, "elevation's {root}");
            (step, w.digest(start, &rule))
        })
        .collect()
}
