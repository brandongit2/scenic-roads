//! The drop boxes are read only through their accessor (docs/inputs.md §7.1): `pipeline::inputs`
//! (`inputs::open`, and the gate's check job, `inputs::gate::run`), and in Python `dem/inputs.py`'s
//! `arg`. Every item outside `#[cfg(test)]` code whose text names a drop box (`MARKERS`, and each
//! gate unit's name) calls the accessor, or is listed in `EXEMPT` by name with why: the writers
//! (the Regions panel's and `scenic add`'s), the backups, GC's never-swept list, the records'
//! names, and the readers each input's task moves behind the accessor (#135–#145, removing their
//! exemptions; #146 checks none is left but the writers'). A new reader of a drop box fails here
//! until it's routed through the accessor; an exemption that no longer matches a line fails too, so
//! the list can't go stale. At run time the NAS root the steps get refuses a drop-box path asked of
//! it by content name (`out::Out::path`).

use std::path::{Path, PathBuf};

/// What names a drop box in Rust (in a line with `sources/inputs` taken out: the checked copies
/// aren't a drop box).
const MARKERS: &[&str] = &["\"inputs", "join(\"inputs\")", "/inputs/", "\"translations", "}/translations", "\"descriptions", "}/descriptions"];

/// What names a drop box in the Python steps (likewise).
const PY_MARKERS: &[&str] = &["\"inputs", "'inputs", "/ \"inputs\"", "/ 'inputs'", "/inputs/", "\"translations", "'translations", "\"descriptions", "'descriptions"];

/// Whether line `l` (its code) names a drop box by `markers` or a unit's name.
fn names_a_drop_box(l: &str, markers: &[&str], units: &[String]) -> bool {
    let l = l.replace("sources/inputs", "");
    markers.iter().any(|m| l.contains(m)) || units.iter().any(|m| l.contains(m.as_str()))
}

/// The accessor's calls: an item making one may name a drop box.
const ACCESSOR: &[&str] = &["inputs::open(", "inputs::gate::run("];

/// The Python accessor's call.
const PY_ACCESSOR: &[&str] = &["inputs.arg("];

/// Items' lines that name a drop box without reading one through the accessor: by file (from the
/// repository's root), item name (a function's, or a `const`'s or `static`'s) and what the line
/// says (an anchor, so a common name like `new` exempts that line alone, and goes stale with it),
/// with why.
const EXEMPT: &[(&str, &str, &str, &str)] = &[
    ("crates/pipeline/src/coverage.rs", "file_rings", "\"inputs/outlines/{f}\"", "the regions' recipes or outline files [#135]"),
    ("crates/pipeline/src/trees/task.rs", "spec", "\"inputs\": list", "a task's input files (its spec's \"inputs\"), not a drop box"),
    ("crates/pipeline/src/namestodo.rs", "run", "\"descriptions\"", "the descriptions' folder and the to-do lists [#137]"),
    ("crates/pipeline/src/namestodo.rs", "run", "\"translations/todo\"", "the descriptions' folder and the to-do lists [#137]"),
    ("crates/pipeline/src/namestodo.rs", "run", "\"descriptions/todo\"", "the descriptions' folder and the to-do lists [#137]"),
    ("crates/pipeline/src/bin/scenic-build.rs", "step_main", "\"--translations\"", "names-todo's --translations default and its folders [#137]; the `inputs` step's name"),
    ("crates/pipeline/src/bin/scenic-build.rs", "step_main", "\"inputs\" => inputs_step(", "names-todo's --translations default and its folders [#137]; the `inputs` step's name"),
    ("crates/pipeline/src/bin/scenic-build.rs", "catalog_coverage", "\"inputs/regions\"", "the regions' recipes or outline files [#135]"),
    ("crates/pipeline/src/bin/scenic-build.rs", "catalog_coverage", "\"inputs/outlines\"", "the regions' recipes or outline files [#135]"),
    ("crates/pipeline/src/bin/scenic-build.rs", "rekey_check", "\"inputs/regions\"", "the regions' recipes or outline files [#135]"),
    ("crates/pipeline/src/bin/scenic-build.rs", "rekey_check", "\"inputs/outlines\"", "the regions' recipes or outline files [#135]"),
    ("crates/pipeline/src/bin/scenic-build.rs", "p5_check_terrain", "\"inputs/regions\"", "the regions' recipes or outline files [#135]"),
    ("crates/pipeline/src/bin/scenic-build.rs", "p5_check_terrain", "\"inputs/outlines\"", "the regions' recipes or outline files [#135]"),
    ("crates/pipeline/src/bin/scenic-build.rs", "water_key_check", "\"inputs/regions\"", "the regions' recipes or outline files [#135]"),
    ("crates/pipeline/src/bin/scenic-build.rs", "water_key_check", "\"inputs/outlines\"", "the regions' recipes or outline files [#135]"),
    ("crates/pipeline/src/bin/scenic-build.rs", "p5_check", "\"inputs/regions\"", "the regions' recipes or outline files [#135]"),
    ("crates/pipeline/src/bin/scenic-build.rs", "p5_check", "\"inputs/outlines\"", "the regions' recipes or outline files [#135]"),
    ("crates/pipeline/src/bin/scenic-build.rs", "pois_step", "\"inputs/regions\"", "the regions' recipes or outline files [#135], --regions' default"),
    ("crates/pipeline/src/bin/scenic-build.rs", "pois_step", "\"inputs/outlines\"", "the regions' recipes or outline files [#135], --regions' default"),
    ("crates/pipeline/src/bin/scenic-build.rs", "pass_and_coverage", "\"inputs/regions\"", "the regions' recipes or outline files [#135], --regions' default"),
    ("crates/pipeline/src/bin/scenic-build.rs", "pass_and_coverage", "\"inputs/outlines\"", "the regions' recipes or outline files [#135], --regions' default"),
    ("crates/pipeline/src/bin/scenic-build.rs", "coverage_of", "\"inputs/regions\"", "the regions' recipes or outline files [#135], --regions' default"),
    ("crates/pipeline/src/bin/scenic-build.rs", "coverage_of", "\"inputs/outlines\"", "the regions' recipes or outline files [#135], --regions' default"),
    ("crates/pipeline/src/bin/scenic-build.rs", "rail_feeds_step", "\"inputs/keys.env\"", "inputs/keys.env, not an input (§2): its key names enter keys, never its values"),
    ("crates/pipeline/src/bin/terrain.rs", "scan", "\"inputs/regions\"", "the regions' recipes or outline files [#135]"),
    ("crates/pipeline/src/bin/terrain.rs", "scan", "\"inputs/outlines\"", "the regions' recipes or outline files [#135]"),
    ("crates/pipeline/src/bin/scenic.rs", "main", "recipes::add(&root(&args, true)?.join(\"inputs/regions\")", "`scenic add`: the owner's tool's write of a recipe (§2)"),
    ("crates/pipeline/src/bin/scenic.rs", "main", "recipes::remove(&root(&args, true)?.join(\"inputs/regions\")", "`scenic remove`: the owner's tool's write of a recipe (§2)"),
    ("crates/pipeline/src/bin/scenic.rs", "main", "\"inputs\" => inputs(&args)", "the `inputs` command's name"),
    ("crates/pipeline/src/bld/task.rs", "spec", "\"inputs\": list", "a task's input files (its spec's \"inputs\"), not a drop box"),
    ("crates/pipeline/src/terrain_task.rs", "spec", "\"inputs\": [[", "a task's input files (its spec's \"inputs\"), not a drop box"),
    ("crates/pipeline/src/agent/gc.rs", "NEVER", "\"translations\"", "GC's never-swept list (§4.9)"),
    ("crates/pipeline/src/agent/steps.rs", "TABLE", "base(\"inputs\", w_inputs)", "the `inputs` step's row"),
    ("crates/pipeline/src/agent/build.rs", "record", "matches!(step, \"inputs\" | ", "the `inputs` step's name"),
    ("crates/pipeline/src/agent/build.rs", "label", "\"inputs\" => \"Checking the inputs dropped\"", "the `inputs` step's name"),
    ("crates/pipeline/src/agent/backup.rs", "FOLDERS", "\"translations\"", "the backups: every drop box and the acceptances, by design (§4.9)"),
    ("crates/pipeline/src/agent/mod.rs", "step", "\"inputs/regions\"", "the regions' recipes or outline files [#135]"),
    ("crates/pipeline/src/agent/mod.rs", "finished", "\"inputs \"", "the `inputs` step's job ids"),
    ("crates/pipeline/src/agent/mod.rs", "plan", "w.step == \"inputs\"", "the `inputs` step's name; inputs/hold-catalog, a control (§2)"),
    ("crates/pipeline/src/agent/mod.rs", "region_work", "\"inputs/regions\"", "the regions' recipes or outline files [#135]; inputs/hold-catalog and keys.env (not inputs, §2)"),
    ("crates/pipeline/src/agent/mod.rs", "region_work", "\"inputs/hold-catalog\"", "the regions' recipes or outline files [#135]; inputs/hold-catalog and keys.env (not inputs, §2)"),
    ("crates/pipeline/src/agent/mod.rs", "region_work", "\"inputs/keys.env can't be read now\"", "the regions' recipes or outline files [#135]; inputs/hold-catalog and keys.env (not inputs, §2)"),
    ("crates/pipeline/src/agent/mod.rs", "checklist", "\"inputs/hold-catalog\"", "the regions' recipes or outline files [#135]; inputs/hold-catalog, a control"),
    ("crates/pipeline/src/agent/mod.rs", "rekey_records", "\"inputs/regions\"", "the regions' recipes or outline files [#135]"),
    ("crates/pipeline/src/agent/mod.rs", "as_read", "\"inputs/regions\"", "the regions' recipes or outline files [#135]"),
    ("crates/pipeline/src/agent/mod.rs", "coverage", "\"inputs/outlines\"", "the regions' recipes or outline files [#135]"),
    ("crates/pipeline/src/agent/mod.rs", "gate_work", "w.step == \"inputs\"", "the `inputs` step's jobs, targets and the agent's own inputs-listing/ folder; the check job reads the drop box"),
    ("crates/pipeline/src/agent/mod.rs", "gate_work", "\"inputs\".into()", "the `inputs` step's jobs, targets and the agent's own inputs-listing/ folder; the check job reads the drop box"),
    ("crates/pipeline/src/agent/mod.rs", "gate_work", "strip_prefix(\"inputs/\")", "the `inputs` step's jobs, targets and the agent's own inputs-listing/ folder; the check job reads the drop box"),
    ("crates/pipeline/src/agent/mod.rs", "gate_work", "\"inputs {unit} full\"", "the `inputs` step's jobs, targets and the agent's own inputs-listing/ folder; the check job reads the drop box"),
    ("crates/pipeline/src/agent/mod.rs", "gate_work", "\"inputs/{unit}\"", "the `inputs` step's jobs, targets and the agent's own inputs-listing/ folder; the check job reads the drop box"),
    ("crates/pipeline/src/agent/mod.rs", "gate_work", "\"inputs-listing\"", "the `inputs` step's jobs, targets and the agent's own inputs-listing/ folder; the check job reads the drop box"),
    ("crates/pipeline/src/agent/mod.rs", "input_digests", "\"inputs/ferries/freq\"", "inputs/keys.env's key names (not an input, §2); the ferries' timetables [#140, #141]"),
    ("crates/pipeline/src/agent/mod.rs", "input_digests", "\"inputs/keys.env\"", "inputs/keys.env's key names (not an input, §2); the ferries' timetables [#140, #141]"),
    ("crates/pipeline/src/agent/mod.rs", "regions_digest", "\"inputs/regions\"", "the regions' recipes or outline files [#135]"),
    ("crates/pipeline/src/agent/mod.rs", "regions_digest", "\"inputs/outlines\"", "the regions' recipes or outline files [#135]"),
    ("crates/pipeline/src/agent/mod.rs", "regions_digest", "\"inputs/outlines/geofabrik\"", "the regions' recipes or outline files [#135]"),
    ("crates/pipeline/src/offload.rs", "offer", "\"inputs\": list", "a task's input files (its spec's \"inputs\"), not a drop box"),
    ("crates/pipeline/src/offload.rs", "run_task", "spec[\"inputs\"]", "a task's input files (its spec's \"inputs\"), not a drop box"),
    ("crates/pipeline/src/ovconv.rs", "ferries_job", "\"inputs/ferries/freq\"", "inputs/ferries/freq [#140, #141]"),
    ("crates/server/src/regions.rs", "nas", "\"inputs/regions\"", "the Regions panel: its writes (the owner's tool, §2) and its reads of the recipes [#135]"),
    ("crates/server/src/descriptions.rs", "new", "\"descriptions\"", "the map server's copy of the descriptions [#137]"),
    ("crates/server/src/descriptions.rs", "load", "\"descriptions: {} from {} files\"", "the map server's copy of the descriptions: its log's words [#137]"),
    ("crates/server/src/descriptions.rs", "spawn", "\"descriptions\"", "the map server's copy of the descriptions [#137]"),
    ("crates/server/src/descriptions.rs", "spawn", "\"descriptions: {e:#}\"", "the map server's copy of the descriptions [#137]"),
    ("crates/server/src/names_live.rs", "new", "\"translations\"", "the map server's copy of the translations [#137]"),
    ("crates/server/src/names_live.rs", "spawn", "\"translations: {e:#}\"", "the map server's copy of the translations: its log's words [#137]"),
    ("crates/server/src/names_live.rs", "reload", "\"translations: {w}\"", "the map server's copy of the translations: its log's words [#137]"),
    ("crates/server/src/names_live.rs", "reload", "\"translations: {} lines, {} names in {} files, languages {:?} ({:.1?})\"", "the map server's copy of the translations: its log's words [#137]"),
    ("crates/server/src/names_live.rs", "reload", "\"translations: WARNING: {ONLY_OLD}\"", "the map server's copy of the translations: its log's words [#137]"),
    ("crates/server/src/names_live.rs", "reload", "\"translations: {e:#}\"", "the map server's copy of the translations: its log's words [#137]"),
    ("crates/server/src/names_live.rs", "sync", "\"translations\"", "the map server's copy of the translations [#137]"),
];

/// The same of the Python steps.
const PY_EXEMPT: &[(&str, &str, &str, &str)] = &[
    ("dem/railfeeds.py", "keys", "\"inputs/keys.env's keys (KEY=value lines; # comments).\"", "inputs/keys.env, not an input (§2): named in its docstring"),
    ("dem/bldfetch.py", "coverage", "\"inputs/outlines\"", "inputs/outlines' .poly files for bld-fetch's coverage [#135]"),
    ("dem/bldfetch.py", "coverage", "\"inputs/outlines/geofabrik\"", "inputs/outlines' .poly files for bld-fetch's coverage [#135]"),
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn files(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            if !matches!(p.file_name().and_then(|n| n.to_str()), Some("target" | "tests" | ".venv" | "node_modules")) {
                files(&p, ext, out);
            }
        } else if p.extension().is_some_and(|x| x == ext) {
            out.push(p);
        }
    }
}

/// A Rust line's code: its string and character literals' contents blanked and its comment cut, so
/// braces can be counted and an accessor call named in a string isn't one.
fn code_of(l: &str) -> String {
    let b: Vec<char> = l.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut in_str = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            if c == '\\' {
                i += 2;
                continue;
            }
            if c == '"' {
                in_str = false;
            }
            out.push(if c == '"' { c } else { ' ' });
        } else if c == '/' && b.get(i + 1) == Some(&'/') {
            break;
        } else if c == '"' {
            in_str = true;
            out.push(c);
        } else if c == '\'' && (b.get(i + 2) == Some(&'\'') || b.get(i + 1) == Some(&'\\')) {
            // (A character literal: '{', '\n'.)
            let end = (i + 1..b.len()).find(|&k| b[k] == '\'' && k > i + 1).unwrap_or(i);
            i = end + 1;
            continue;
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// Where an item starting at line `at` ends: a function's block by its braces, a `const`'s or a
/// `static`'s at the `;` that closes all it opened (brackets and parentheses too).
fn item_end(code: &[String], at: usize, is_fn: bool) -> usize {
    if is_fn {
        return block_end(code, at);
    }
    let mut depth = 0i64;
    for (k, l) in code.iter().enumerate().skip(at) {
        for ch in l.chars() {
            match ch {
                '{' | '[' | '(' => depth += 1,
                '}' | ']' | ')' => depth -= 1,
                _ => {}
            }
        }
        if depth <= 0 && l.trim_end().ends_with(';') {
            return k;
        }
    }
    code.len().saturating_sub(1)
}

/// The line where the block opened at or after line `at` closes (braces counted on `code`).
fn block_end(code: &[String], at: usize) -> usize {
    let mut depth = 0i64;
    let mut opened = false;
    for (k, l) in code.iter().enumerate().skip(at) {
        for ch in l.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    opened = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        if opened && depth <= 0 {
            return k;
        }
        // (An item without a block: a `const` or a `fn` declared, ending at its `;`.)
        if !opened && l.trim_end().ends_with(';') {
            return k;
        }
    }
    code.len().saturating_sub(1)
}

/// The lines inside `#[cfg(test)]` items (a test module, a test helper), each attribute's item to
/// its block's end.
fn test_lines(code: &[String]) -> Vec<bool> {
    let mut mask = vec![false; code.len()];
    let mut k = 0;
    while k < code.len() {
        let t = code[k].trim();
        if t.starts_with("#[cfg(test)]") || t.starts_with("#[cfg(all(test") {
            let end = block_end(code, k + 1);
            for m in mask.iter_mut().take(end + 1).skip(k) {
                *m = true;
            }
            k = end + 1;
        } else {
            k += 1;
        }
    }
    mask
}

/// The name an item line starts (a function's, `const fn` and `async fn` too; a `const`'s or a
/// `static`'s), if it starts one.
fn item_name(l: &str) -> Option<(String, bool)> {
    let mut t = l.trim_start();
    for p in ["pub(crate) ", "pub(super) ", "pub "] {
        t = t.strip_prefix(p).unwrap_or(t);
    }
    for p in ["const fn ", "async fn ", "unsafe fn ", "fn "] {
        if let Some(r) = t.strip_prefix(p) {
            return Some((r.split(|c: char| !(c.is_alphanumeric() || c == '_')).next().unwrap_or("").to_string(), true));
        }
    }
    for p in ["const ", "static "] {
        if let Some(r) = t.strip_prefix(p) {
            return Some((r.split(|c: char| !(c.is_alphanumeric() || c == '_')).next().unwrap_or("").to_string(), false));
        }
    }
    None
}

/// The innermost item line `i` lies in (a function, else a `const` or `static`), by its block:
/// its name and its code; None at the top level of a file, outside any.
fn enclosing_rs(code: &[String], i: usize) -> Option<(String, String)> {
    let mut best: Option<(usize, usize, String)> = None;
    for k in (0..=i).rev() {
        let Some((name, is_fn)) = item_name(&code[k]) else { continue };
        let end = item_end(code, k, is_fn);
        if end >= i && best.as_ref().is_none_or(|b| k > b.0) {
            best = Some((k, end, name));
            // (The nearest that holds it is the innermost.)
            break;
        }
    }
    best.map(|(k, end, name)| (name, code[k..=end].join("\n")))
}

/// The Python function or top-level assignment line `i` lies in (by indentation), its name and its
/// code (comments cut); None outside any.
fn enclosing_py(lines: &[&str], i: usize) -> Option<(String, String)> {
    let indent = |l: &str| l.len() - l.trim_start().len();
    let code = |l: &str| py_code(l);
    for k in (0..=i).rev() {
        let t = lines[k].trim_start();
        if let Some(r) = t.strip_prefix("def ").or_else(|| t.strip_prefix("async def ")) {
            let end = (k + 1..lines.len()).find(|&m| !lines[m].trim().is_empty() && indent(lines[m]) <= indent(lines[k])).unwrap_or(lines.len());
            if end > i {
                let name = r.split(|c: char| !(c.is_alphanumeric() || c == '_')).next().unwrap_or("").to_string();
                return Some((name, lines[k..end].iter().map(|l| code(l)).collect::<Vec<_>>().join("\n")));
            }
        }
        if indent(lines[k]) == 0 && !t.is_empty() && !t.starts_with('#') {
            // (A top-level statement: an assignment's name.)
            let name = t.split(|c: char| !(c.is_alphanumeric() || c == '_')).next().unwrap_or("").to_string();
            return (!name.is_empty() && k == i || t.contains('=')).then(|| (name, code(lines[k])));
        }
    }
    None
}

/// A Python line's code: its comment cut and its string literals' contents blanked.
fn py_code(l: &str) -> String {
    let mut out = String::new();
    let mut q: Option<char> = None;
    let mut esc = false;
    for c in l.chars() {
        match q {
            Some(open) => {
                if esc {
                    esc = false;
                } else if c == '\\' {
                    esc = true;
                } else if c == open {
                    q = None;
                    out.push(c);
                    continue;
                }
                out.push(' ');
            }
            None if c == '#' => break,
            None => {
                if c == '"' || c == '\'' {
                    q = Some(c);
                }
                out.push(c);
            }
        }
    }
    out
}

/// Each gate unit's name as code would write it in a path.
fn unit_markers() -> Vec<String> {
    pipeline::inputs::UNITS.iter().chain([&pipeline::inputs::TEST_UNIT]).map(|u| format!("\"{u}")).collect()
}

/// The lines naming a drop box outside the accessor, and the exemptions that matched none.
fn scan() -> (Vec<String>, Vec<String>) {
    let root = root();
    let units = unit_markers();
    let mut bad = Vec::new();
    let mut used = std::collections::BTreeSet::<usize>::new();
    let mut rs = Vec::new();
    files(&root.join("crates"), "rs", &mut rs);
    for f in rs {
        let rel = f.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        // (The accessor itself: the gate.)
        if rel.starts_with("crates/pipeline/src/inputs/") {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let code: Vec<String> = lines.iter().map(|l| code_of(l)).collect();
        let tests = test_lines(&code);
        for i in 0..lines.len() {
            let l = lines[i];
            // (A marker in code, not in a comment.)
            let c = l.split("//").next().unwrap_or("");
            if tests[i] || !names_a_drop_box(c, MARKERS, &units) {
                continue;
            }
            let (name, body) = enclosing_rs(&code, i).unwrap_or_else(|| ("<top level>".into(), String::new()));
            if ACCESSOR.iter().any(|a| body.contains(a)) {
                continue;
            }
            if let Some(k) = EXEMPT.iter().position(|(ef, en, anchor, _)| *ef == rel && *en == name && l.contains(anchor)) {
                used.insert(k);
                continue;
            }
            bad.push(format!("{rel}:{} in {name}: {}", i + 1, l.trim()));
        }
    }
    let mut py = Vec::new();
    files(&root.join("dem"), "py", &mut py);
    let mut py_used = std::collections::BTreeSet::<usize>::new();
    for f in py {
        let rel = f.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        if rel == "dem/inputs.py" {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        for (i, l) in lines.iter().enumerate() {
            // (A marker in a string or code, a comment cut.)
            let c = l.split(" #").next().unwrap_or("");
            if c.trim_start().starts_with('#') || !names_a_drop_box(c, PY_MARKERS, &units) {
                continue;
            }
            let (name, body) = enclosing_py(&lines, i).unwrap_or_else(|| ("<top level>".into(), String::new()));
            if PY_ACCESSOR.iter().any(|a| body.contains(a)) {
                continue;
            }
            if let Some(k) = PY_EXEMPT.iter().position(|(ef, en, anchor, _)| *ef == rel && *en == name && l.contains(anchor)) {
                py_used.insert(k);
                continue;
            }
            bad.push(format!("{rel}:{} in {name}: {}", i + 1, l.trim()));
        }
    }
    let unused: Vec<String> = EXEMPT.iter().enumerate().filter(|(k, _)| !used.contains(k)).chain(PY_EXEMPT.iter().enumerate().filter(|(k, _)| !py_used.contains(k))).map(|(_, (f, n, a, _))| format!("{f}: {n}: {a}")).collect();
    (bad, unused)
}

#[test]
fn every_drop_box_read_goes_through_the_accessor() {
    let (bad, _) = scan();
    assert!(bad.is_empty(), "drop boxes named outside their accessor (inputs::open, inputs::gate::run; dem/inputs.py's arg); route the read through it, or list the item in EXEMPT with why:\n{}", bad.join("\n"));
}

/// Every exemption still matches a line it exempts (one that doesn't would let a new reader by
/// under its name).
#[test]
fn every_exemption_matches_a_line() {
    let (_, unused) = scan();
    assert!(unused.is_empty(), "exemptions matching no line naming a drop box (remove them):\n{}", unused.join("\n"));
}

/// The scanner's own rules: an item's block by its braces (a brace in a string or a character
/// literal aside), `const fn` and a top-level `const` named, `#[cfg(test)]` items skipped, a line
/// outside any item at the top level.
#[test]
fn the_scanner_finds_items_by_their_blocks() {
    let src = r#"const A: &str = "inputs/x";
#[cfg(test)]
fn helper() { let _ = "inputs/y"; }
pub const fn b() -> &'static str {
    let _c = '{';
    "inputs/z"
}
fn c() {
    let s = "{";
}
static D: [&str; 1] = ["translations"];"#;
    let code: Vec<String> = src.lines().map(code_of).collect();
    let tests = test_lines(&code);
    assert_eq!(tests, [false, true, true, false, false, false, false, false, false, false, false]);
    assert_eq!(enclosing_rs(&code, 0).map(|e| e.0).as_deref(), Some("A"));
    assert_eq!(enclosing_rs(&code, 5).map(|e| e.0).as_deref(), Some("b"));
    assert_eq!(enclosing_rs(&code, 8).map(|e| e.0).as_deref(), Some("c"));
    assert_eq!(enclosing_rs(&code, 10).map(|e| e.0).as_deref(), Some("D"));
    // (An accessor call named in a string isn't one; a marker with `sources/inputs` out of it.)
    assert!(!code_of("let s = \"inputs::open(\";").contains("inputs::open("));
    assert!(!py_code("x = 'inputs.arg(p)'  # inputs.arg(").contains("inputs.arg("));
    assert!(names_a_drop_box("root.join(\"x/inputs/y\")", MARKERS, &[]) && !names_a_drop_box("\"sources/inputs/x\"", MARKERS, &[]));
    assert!(names_a_drop_box("p = root / 'inputs' / 'x'", PY_MARKERS, &[]));
    let py = ["X = Path(\"inputs\")", "def f():", "    return \"inputs/a\"", "", "def g():", "    pass"];
    assert_eq!(enclosing_py(&py, 0).map(|e| e.0).as_deref(), Some("X"));
    assert_eq!(enclosing_py(&py, 2).map(|e| e.0).as_deref(), Some("f"));
}

/// The NAS root a step gets refuses a drop-box path at run time, naming the caller, whatever builds
/// it; the accepted copies and the rest of the root it gives.
#[test]
fn the_steps_root_refuses_a_drop_box_path() {
    let d = tempfile::tempdir().unwrap();
    let out = pipeline::out::Out::open(d.path(), &d.path().join("s")).unwrap();
    for p in ["inputs/regions/wales.toml", "inputs/_gate-test/a.jsonl", "translations/fr.jsonl", "descriptions/x.jsonl", "inputs", "./inputs/keys.env"] {
        let e = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| out.path(p))).expect_err(p);
        let msg = e.downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(msg.contains("inputs_accessor.rs") && msg.contains("drop box"), "{p}: {msg}");
    }
    for p in ["sources/inputs/_gate-test/a.0123456789abcdef.jsonl", "base/6-1-1.0123456789abcdef.base", "inputsx"] {
        assert_eq!(out.path(p), d.path().join(p));
    }
}

/// dem/inputs.py's `arg`: an input's path a Python step is given, refused in a drop box (anything
/// not under `sources/inputs/`, `sources/fetched/` or the job's scratch that lies under `inputs/`,
/// `translations/` or `descriptions/`). (Passed over, said, without python3.)
#[test]
fn the_python_steps_open_inputs_through_their_helper() {
    let ok = std::process::Command::new("python3").args(["-I", "-c", "pass"]).status().is_ok_and(|s| s.success());
    if !ok {
        eprintln!("python3 isn't here: dem/inputs.py not run");
        return;
    }
    let code = r#"
import sys
sys.path.insert(0, sys.argv[1])
import inputs
for p in ["/nas/inputs/regions/wales.toml", "/nas/translations/fr.jsonl", "/nas/descriptions/x.jsonl", "inputs/_gate-test/a.jsonl"]:
    try:
        inputs.arg(p)
    except inputs.DropBox:
        continue
    raise SystemExit(f"not refused: {p}")
for p in ["/nas/sources/inputs/_gate-test/a.0123456789abcdef.jsonl", "/nas/sources/fetched/x/y.zip", "/scratch/job/inputs.json", "/nas/base/6-1-1.0123456789abcdef.base"]:
    assert str(inputs.arg(p)) == p, p
print("ok")
"#;
    let out = std::process::Command::new("python3").arg("-I").arg("-c").arg(code).arg(root().join("dem")).output().unwrap();
    assert!(out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "ok", "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
}
