//! The steps reading the shared cache, agentcore::needs::CACHES (task #1173), held to the code
//! that opens it: the whole local copies (buildstore::localread::Local) are opened by
//! `Local::from_env` or `Local::new` (by any path to `Local`), and in scenic-build every such call
//! sits in a function the step dispatch calls, its arm a step's (`"a" | "b" => f(…)`, the arm on one
//! line or several, `f` called anywhere in it). So the steps whose functions open them are the
//! cache's steps, exactly: a step opening the copies unlisted fails here, as does one listed that
//! no longer opens them, and a call outside every dispatched function (a helper of its own) fails
//! until it's moved into one. Outside scenic-build, the copies are opened only in the files
//! `ELSEWHERE` names, each with why what it opens there isn't the shared cache
//! (buildstore::localread::is_shared); a new caller fails here until it's a step's (and listed) or
//! named with its reason. `Local` taken in under another name (`use … Local as …`) fails too, so no
//! call goes unseen.
//!
//! scans: crates

mod common;

use std::collections::{BTreeMap, BTreeSet};

/// The calls that open the copies (after `Local::`, by any path to it).
const OPENS: [&str; 2] = ["Local::from_env(", "Local::new("];

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

/// Whether a line (its code, `common::code_of`) opens the copies: `Local::from_env(` or
/// `Local::new(`, `Local` a whole word.
fn opens(l: &str) -> bool {
    OPENS.iter().any(|p| {
        l.match_indices(p).any(|(at, _)| !l[..at].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_'))
    })
}

/// The functions a file defines, by the line its `fn` opens on.
fn functions(code: &[String]) -> BTreeMap<usize, String> {
    let mut out = BTreeMap::new();
    for (k, l) in code.iter().enumerate() {
        let t = l.trim_start();
        let t = t.strip_prefix("pub ").unwrap_or(t);
        let t = t.strip_prefix("async ").unwrap_or(t);
        if let Some(r) = t.strip_prefix("fn ") {
            out.insert(k, r.split(['(', '<']).next().unwrap_or("").trim().to_string());
        }
    }
    out
}

/// Each function of the file scenic-build's step dispatch calls, with the steps whose arms call it:
/// an arm whose patterns are all strings (`raw`, the strings kept), its body from `=>` to its end
/// (a block, or to its `,`, over as many lines as it takes: `code`, the strings blanked), each of
/// the file's functions called in it.
fn dispatched(raw: &[String], code: &[String], fns: &BTreeSet<String>) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (k, l) in raw.iter().enumerate() {
        let Some((pats, _)) = l.trim().split_once("=>") else { continue };
        let steps: Vec<&str> = pats.split('|').map(str::trim).collect();
        if !steps.iter().all(|s| s.len() > 2 && s.starts_with('"') && s.ends_with('"')) {
            continue;
        }
        let Some(at) = code[k].find("=>") else { continue };
        // (The arm's body: its text from `=>`, a block to its close, else until a `,` or the
        // match's `}` at its own depth.)
        let mut body = String::new();
        let mut depth = 0i64;
        let mut block = None;
        'arm: for (j, line) in code.iter().enumerate().skip(k) {
            let from = if j == k { at + 2 } else { 0 };
            for c in line[from..].chars() {
                if block.is_none() && !c.is_whitespace() {
                    block = Some(c == '{');
                }
                match c {
                    '(' | '{' | '[' => depth += 1,
                    ')' | '}' | ']' if depth == 0 => break 'arm,
                    ')' | '}' | ']' => {
                        depth -= 1;
                        if depth == 0 && block == Some(true) {
                            break 'arm;
                        }
                    }
                    ',' if depth == 0 => break 'arm,
                    _ => {}
                }
                body.push(c);
            }
            body.push(' ');
        }
        let names = body.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '(')).flat_map(|w| {
            w.split_inclusive('(').filter_map(|p| p.strip_suffix('(')).map(str::to_string).collect::<Vec<_>>()
        });
        for f in names.filter(|f| fns.contains(f)) {
            out.entry(f).or_default().extend(steps.iter().map(|s| s.trim_matches('"').to_string()));
        }
    }
    out
}

/// What scenic-build's code says: each step whose dispatched function opens the copies, and the
/// lines opening them in no dispatched function.
fn scenic_build_opening(text: &str) -> (BTreeSet<String>, Vec<String>) {
    let raw: Vec<String> = text.lines().map(str::to_string).collect();
    let code: Vec<String> = raw.iter().map(|l| common::code_of(l)).collect();
    let tests = common::test_lines(&code);
    let fns = functions(&code);
    let names: BTreeSet<String> = fns.values().cloned().collect();
    let calls = dispatched(&raw, &code, &names);
    assert!(calls.contains_key("depth_step"), "the dispatch read: {calls:?}");
    let spans: Vec<(usize, usize, &String)> = fns.iter().map(|(&k, f)| (k, common::block_end(&code, k), f)).collect();
    let (mut opening, mut stray) = (BTreeSet::new(), Vec::new());
    for (k, l) in code.iter().enumerate().filter(|(k, l)| !tests[*k] && opens(l)) {
        // (Every function it sits in, nested or not: one of them dispatched.)
        let steps: Vec<&BTreeSet<String>> =
            spans.iter().filter(|(a, b, _)| *a <= k && k <= *b).filter_map(|(_, _, f)| calls.get(*f)).collect();
        if steps.is_empty() {
            stray.push(format!("line {}: {}", k + 1, l.trim()));
        }
        opening.extend(steps.into_iter().flatten().cloned());
    }
    (opening, stray)
}

#[test]
fn the_shared_caches_steps_are_those_whose_jobs_open_it() {
    let root = common::root();
    let text = std::fs::read_to_string(root.join("crates/pipeline/src/bin/scenic-build.rs")).unwrap();
    let (opening, stray) = scenic_build_opening(&text);
    assert!(
        stray.is_empty(),
        "scenic-build opens the whole copies outside every function its step dispatch calls (move the call into the \
         step's function, so the step it's for is known): {stray:?}"
    );
    let listed: BTreeSet<String> =
        agentcore::needs::CACHES.iter().flat_map(|(_, steps)| steps.iter().map(|s| s.to_string())).collect();
    assert_eq!(
        opening, listed,
        "agentcore::needs::CACHES lists the steps whose jobs open the whole copies (scenic-build's step functions \
         calling {OPENS:?}): make it these"
    );
}

/// The scan itself: a call in a helper no arm names is stray; one through an imported `Local` or
/// by `Local::new`, in a function a multi-line arm or a block arm calls, is its steps'.
#[test]
fn the_scan_reads_every_call_by_any_path_and_every_arm() {
    let text = r#"
fn main() {
    match step.as_str() {
        "one" | "two" => {
            let x = 1;
            first(x)?
        }
        "three" =>
            second(
                &out,
            )?,
        "four" => helper_free(),
        "depth" => depth_step(),
        _ => {}
    }
}
fn first(x: u32) -> Result<()> {
    let local = Local::from_env(root);
    Ok(())
}
fn second(out: &Out) -> Result<()> {
    let local = buildstore::localread::Local::new(root, &cache, 0);
    Ok(())
}
fn helper() {
    let local = Local::from_env(root);
}
fn helper_free() {}
fn depth_step() {}
"#;
    let (opening, stray) = scenic_build_opening(text);
    assert_eq!(opening, ["one", "two", "three"].map(String::from).into());
    let helper = text.lines().position(|l| l == "fn helper() {").unwrap() + 2;
    assert_eq!(stray, [format!("line {helper}: let local = Local::from_env(root);")], "the helper's call alone");
    assert!(opens("let l = Local::new(a, b, 0);") && !opens("let l = CacheLocal::new();"));
}

#[test]
fn the_copies_are_opened_elsewhere_only_for_what_isnt_the_shared_cache() {
    let root = common::root();
    let mut rs = Vec::new();
    common::files(&root.join("crates"), "rs", &mut rs);
    let (mut found, mut renamed) = (BTreeSet::new(), BTreeSet::new());
    for f in rs {
        let rel = f.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        let text = std::fs::read_to_string(&f).unwrap();
        let code: Vec<String> = text.lines().map(common::code_of).collect();
        if code.iter().any(|l| l.contains("localread::") && l.contains("Local as ")) {
            renamed.insert(rel.clone());
        }
        // (A module of tests in its own file, `tests.rs` or `<name>_tests.rs`, is a test's.)
        let test_file = rel.ends_with("/tests.rs") || rel.ends_with("_tests.rs");
        if test_file || rel == "crates/pipeline/src/bin/scenic-build.rs" || rel == "crates/buildstore/src/localread.rs"
        {
            continue;
        }
        let tests = common::test_lines(&code);
        if code.iter().enumerate().any(|(k, l)| !tests[k] && opens(l)) {
            found.insert(rel);
        }
    }
    assert!(
        renamed.is_empty(),
        "localread's Local taken in under another name, which the scans can't follow: {renamed:?}"
    );
    let named: BTreeSet<String> = ELSEWHERE.iter().map(|(f, _)| f.to_string()).collect();
    assert_eq!(
        found, named,
        "{OPENS:?} outside scenic-build's steps: a step's job opening the shared cache belongs in its step function \
         (and agentcore::needs::CACHES); a reader of other copies goes in ELSEWHERE with why"
    );
}
