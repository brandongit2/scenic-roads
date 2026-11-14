//! scans: crates/ dem/ web/buildpage/sums.js
//!
//! The phases' names the build page describes and the renames records are read through stay true of
//! the code (task #1132): each of the page's descriptions (web/buildpage/sums.js `PHASE_ABOUT`) is
//! of a phase some step emits now, and each rename (jobrt::phase_names::RENAMED) points at a name a
//! step emits now, which the page describes. So a phase renamed in the code fails here until its
//! description moves to the new name, and a rename's row until its new name is the code's.
//!
//! Phases are named by free strings where they open (jobrt::timings::phase and sub, and the
//! wrappers taking a name: roads' stages, a program's or script's run, the OSM pass's parts), so
//! what the code emits is read from its source, grepped: a name is emitted when it's a string
//! literal in a crate's code (its `src/`, outside test modules and test files) or a Python
//! script's under dem/, or when it fits a name a phase call builds with `format!` (`sampling {}`,
//! `{ph} (osmium)`). A literal that isn't a phase's passes too: the check is that the name is the
//! code's, not where it's opened.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn files(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if name != "tests" && name != "target" && !name.starts_with('.') {
                files(&p, ext, out);
            }
        } else if p.extension().is_some_and(|x| x == ext) && name != "tests.rs" {
            out.push(p);
        }
    }
}

/// The crates' code (each `src/`, test modules cut: an inline `#[cfg(test)] mod … {` and what
/// follows it, the repo's way of keeping them last) and dem/'s Python, but this crate's
/// phase_names.rs (the old names).
fn code() -> String {
    let r = root();
    let mut fs = Vec::new();
    for c in std::fs::read_dir(r.join("crates")).unwrap().flatten() {
        if c.path().join("src").is_dir() {
            files(&c.path().join("src"), "rs", &mut fs);
        }
    }
    let mut py = Vec::new();
    files(&r.join("dem"), "py", &mut py);
    let mut all = String::new();
    for f in fs.iter().filter(|f| !f.ends_with("jobrt/src/phase_names.rs")) {
        let s = std::fs::read_to_string(f).unwrap();
        let cut = s
            .match_indices("#[cfg(test)]\nmod ")
            .find(|(i, _)| s[*i..].lines().nth(1).is_some_and(|l| l.trim_end().ends_with('{')))
            .map_or(s.len(), |(i, _)| i);
        all += &s[..cut];
        all.push('\n');
    }
    for f in py {
        all += &std::fs::read_to_string(f).unwrap();
        all.push('\n');
    }
    all
}

/// The names phase calls build with `format!` (`phase(&format!("sampling {}", …))`), as their
/// literal pieces around the placeholders.
fn built(code: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for call in ["phase(&format!(\"", "sub(&format!(\""] {
        for (i, _) in code.match_indices(call) {
            let s = &code[i + call.len()..];
            let t = &s[..s.find('"').unwrap()];
            let mut pieces = Vec::new();
            let mut rest = t;
            while let Some(a) = rest.find('{') {
                pieces.push(rest[..a].to_string());
                rest = &rest[a + rest[a..].find('}').unwrap() + 1..];
            }
            pieces.push(rest.to_string());
            if pieces.len() > 1 {
                out.push(pieces);
            }
        }
    }
    out
}

/// Whether `name` fits a built name: its pieces in order, something in each placeholder.
fn fits(name: &str, pieces: &[String]) -> bool {
    let (first, last) = (&pieces[0], &pieces[pieces.len() - 1]);
    let Some(mut rest) = name.strip_prefix(first.as_str()) else { return false };
    for p in &pieces[1..pieces.len() - 1] {
        match rest.match_indices(p.as_str()).find(|(i, _)| *i > 0) {
            Some((i, _)) => rest = &rest[i + p.len()..],
            None => return false,
        }
    }
    rest.len() > last.len() && rest.ends_with(last.as_str())
}

/// web/buildpage/sums.js `PHASE_ABOUT`'s keys.
fn described() -> Vec<String> {
    let s = std::fs::read_to_string(root().join("web/buildpage/sums.js")).unwrap();
    let from = s.find("export const PHASE_ABOUT = {\n").expect("sums.js's PHASE_ABOUT");
    let body = &s[from..from + s[from..].find("\n};").unwrap()];
    body.lines()
        .skip(1)
        .map(|l| {
            let l = l.trim_start().strip_prefix('"').unwrap_or_else(|| panic!("a key per line: {l}"));
            l[..l.find("\": \"").unwrap_or_else(|| panic!("a key per line: {l}"))].to_string()
        })
        .collect()
}

#[test]
fn described_and_renamed_phases_are_the_codes() {
    let code = code();
    let built = built(&code);
    assert!(built.iter().any(|p| p == &["sampling ", ""]), "the datasets' sampling phases found: {built:?}");
    let emitted = |n: &str| {
        code.contains(&format!("\"{n}\"")) || code.contains(&format!("'{n}'")) || built.iter().any(|p| fits(n, p))
    };
    let about = described();
    assert!(about.len() > 20, "PHASE_ABOUT read: {about:?}");
    let stale: Vec<&String> = about.iter().filter(|n| *n != jobrt::timings::OTHER_PHASES && !emitted(n)).collect();
    assert!(stale.is_empty(), "web/buildpage/sums.js PHASE_ABOUT describes phases no step emits (renamed: move the line to the new name; gone: cut it): {stale:?}");
    for (step, old, new) in jobrt::phase_names::RENAMED {
        assert_ne!(old, new, "{step}");
        assert!(emitted(new), "jobrt::phase_names: {step}'s `{old}` is read as `{new}`, which no step emits");
        assert!(
            about.iter().any(|n| n == new),
            "{step}'s renamed phase `{new}` has no line in web/buildpage/sums.js PHASE_ABOUT"
        );
    }
}

#[test]
fn a_built_name_fits_its_pieces() {
    let p = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    assert!(fits("sampling glo30", &p(&["sampling ", ""])));
    assert!(!fits("sampling ", &p(&["sampling ", ""])));
    assert!(fits("cutting the boxes (osmium)", &p(&["", " (osmium)"])));
    assert!(!fits(" (osmium)", &p(&["", " (osmium)"])));
    assert!(fits("a x b y", &p(&["a ", " b ", ""])));
    assert!(!fits("a  b y", &p(&["a ", " b ", ""])));
    assert_eq!(
        built("phase(&format!(\"{ph} (osmium)\")) sub(&format!(\"{}–{}\", a, b))"),
        [p(&["", " (osmium)"]), p(&["", "–", ""])]
    );
}
