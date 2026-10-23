//! The drop boxes are read only through their accessor (docs/inputs.md §7.1): `pipeline::inputs`
//! (`inputs::open`, and the gate's check job, `inputs::gate::run`), and in Python `dem/inputs.py`'s
//! `arg`. Every item outside `#[cfg(test)]` code whose text names a drop box (`MARKERS`, and each
//! gate unit's name) calls the accessor, or is listed in `EXEMPT` by name with why: no reader, only
//! the Regions panel's writes, the backups, GC's never-swept list, the controls beside the drop boxes
//! (`inputs/hold-catalog`, `inputs/keys.env`), and words that aren't paths (the `inputs` step's
//! name, a task spec's "inputs", log lines). A new reader of a drop box fails here until it's routed
//! through the accessor; an exemption that no longer matches a line fails too, so
//! the list can't go stale. At run time the NAS root the steps get refuses a drop-box path asked of
//! it by content name (`out::Out::path`).

mod common;

use common::*;

/// What names a drop box in Rust (in a line with `sources/inputs` taken out: the checked copies
/// aren't a drop box).
const MARKERS: &[&str] = &[
    "\"inputs",
    "join(\"inputs\")",
    "/inputs/",
    "\"translations",
    "}/translations",
    "\"descriptions",
    "}/descriptions",
];

/// What names a drop box in the Python steps (likewise).
const PY_MARKERS: &[&str] = &[
    "\"inputs",
    "'inputs",
    "/ \"inputs\"",
    "/ 'inputs'",
    "/inputs/",
    "\"translations",
    "'translations",
    "\"descriptions",
    "'descriptions",
];

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
    ("crates/pipeline/src/agent/every.rs", "LEAD_ONLY", "\"inputs\"", "the `inputs` step's name, kept to the lead, not a drop box"),
    ("crates/pipeline/src/agent/forecast.rs", "CLOCKED", "\"gc\", \"inputs\"", "the `inputs` step's name among the lead's clocked steps, not a drop box"),
    ("crates/pipeline/src/trees/task.rs", "spec", "\"inputs\": list", "a task's input files (its spec's \"inputs\"), not a drop box"),
    ("crates/pipeline/src/bld/task.rs", "spec", "\"inputs\": list", "a task's input files (its spec's \"inputs\"), not a drop box"),
    ("crates/pipeline/src/terrain_task.rs", "spec", "\"inputs\": [[", "a task's input files (its spec's \"inputs\"), not a drop box"),
    ("crates/pipeline/src/agent/gc.rs", "NEVER", "\"translations\"", "GC's never-swept list (§4.9): the drop boxes, and the old translations/ and descriptions/ (#137), whose drops the input check flags"),
    ("crates/pipeline/src/agent/steps.rs", "TABLE", "^\"inputs\",\"Input check\"", "the `inputs` step's row"),
    ("crates/pipeline/src/agent/build.rs", "record", "^\"inputs\"| \"outlines\"", "the `inputs` step's name"),
    ("crates/pipeline/src/agent/backup.rs", "FOLDERS", "[\"inputs\",", "the backups: every drop box and the acceptances, by design (§4.9)"),
    ("crates/pipeline/src/agent/mod.rs", "finished", "\"inputs \"", "the `inputs` step's job ids"),
    ("crates/pipeline/src/cli/build.rs", "STEPS", "s(\"inputs", "the `inputs` step's name and usage"),
    ("crates/pipeline/src/agent/mod.rs", "is_input_check", "Some(\"inputs\")", "the `inputs` step's name"),
    ("crates/pipeline/src/agent/mod.rs", "plan_unchecked","w.step == \"inputs\"", "the `inputs` step's name; inputs/hold-catalog, a control (§2)"),
    ("crates/pipeline/src/agent/mod.rs", "region_jobs", "\"inputs/hold-catalog\"", "inputs/hold-catalog and keys.env (not inputs, §2)"),
    ("crates/pipeline/src/agent/mod.rs", "region_jobs", "\"inputs/keys.env can't be read now\"", "inputs/hold-catalog and keys.env (not inputs, §2)"),
    ("crates/pipeline/src/agent/mod.rs", "plan_seen", "\"inputs/hold-catalog\"", "inputs/hold-catalog, a control: only its size and time, for an idle agent's plan digest (task #154)"),
    ("crates/pipeline/src/agent/mod.rs", "checklist", "\"inputs/hold-catalog\"", "inputs/hold-catalog, a control"),
    ("crates/pipeline/examples/checklist.rs", "main", "\"inputs/hold-catalog\"", "inputs/hold-catalog, a control"),
    ("crates/pipeline/src/agent/mod.rs", "gate_work", "w.step == \"inputs\"", "the `inputs` step's jobs, targets and the agent's own inputs-listing/ folder; the check job reads the drop box"),
    ("crates/pipeline/src/agent/mod.rs", "gate_work", "\"inputs\".into()", "the `inputs` step's jobs, targets and the agent's own inputs-listing/ folder; the check job reads the drop box"),
    ("crates/pipeline/src/agent/mod.rs", "gate_work", "strip_prefix(\"inputs/\")", "the `inputs` step's jobs, targets and the agent's own inputs-listing/ folder; the check job reads the drop box"),
    ("crates/pipeline/src/agent/mod.rs", "gate_work", "\"inputs {unit} full\"", "the `inputs` step's jobs, targets and the agent's own inputs-listing/ folder; the check job reads the drop box"),
    ("crates/pipeline/src/agent/mod.rs", "gate_work", "\"inputs/{unit}\"", "the `inputs` step's jobs, targets and the agent's own inputs-listing/ folder; the check job reads the drop box"),
    ("crates/pipeline/src/agent/mod.rs", "gate_work", ".join(\"scratch\").join(\"inputs\")", "the `inputs` step's jobs, targets and the agent's own inputs-listing/ folder; the check job reads the drop box"),
    ("crates/pipeline/src/agent/mod.rs", "gate_work", "\"inputs {unit} forget\"", "the `inputs` step's jobs, targets and the agent's own inputs-listing/ folder; the check job reads the drop box"),
    ("crates/pipeline/src/agent/mod.rs", "gate_work", "\"inputs-listing\"", "the `inputs` step's jobs, targets and the agent's own inputs-listing/ folder; the check job reads the drop box"),
    ("crates/pipeline/src/agent/mod.rs", "input_digests", "\"inputs/keys.env\"", "inputs/keys.env's key names (not an input, §2)"),
    ("crates/pipeline/src/budget/snmp.rs", "KEYS", "\"inputs/keys.env\"", "inputs/keys.env's SNMP keys for the lead's NAS load reads (not an input, §2)"),
    ("crates/pipeline/src/offload.rs", "offer", "\"inputs\": list", "a task's input files (its spec's \"inputs\"), not a drop box"),
    ("crates/pipeline/src/offload.rs", "run_task", "spec[\"inputs\"]", "a task's input files (its spec's \"inputs\"), not a drop box"),
    ("crates/server/src/regions.rs", "nas", "\"inputs/regions\"", "the Regions panel: the owner's tool (§2), its writes and the list it edits"),
    ("crates/server/src/descriptions.rs", "load", "\"descriptions: {} from {} files\"", "the map server's descriptions: its log's words, not a path"),
    ("crates/server/src/names_live.rs", "reload", "\"translations: {w}\"", "the map server's translations: its log's words, not a path"),
    ("crates/server/src/names_live.rs", "reload", "\"translations: {} lines, {} names in {} files, languages {:?} ({:.1?})\"", "the map server's translations: its log's words, not a path"),
    ("crates/server/src/descriptions.rs", "sync", "\"descriptions: {e:#}\"", "the map server's descriptions: its log's words, not a path"),
    ("crates/server/src/names_live.rs", "sync", "\"translations: {e:#}\"", "the map server's translations: its log's words, not a path"),
];

/// Each gate unit's name as code would write it in a path. (The regions', the heritage
/// registers' and matches', the elevation datasets' and the source declarations' aside: "regions",
/// "heritage", "elevation" and "sources" are words every reader of the coverage, the catalog, the
/// heritage chain, the roads' heights and the NAS's stores (`sources/…`) says, and their drop boxes
/// are only ever named `inputs/regions`,
/// `inputs/heritage`, `inputs/heritage-matches`, `inputs/elevation`, `inputs/sources`, which
/// `MARKERS` catches.)
fn unit_markers() -> Vec<String> {
    use pipeline::inputs::{elevation, heritage, heritage_matches, regions, sources};
    pipeline::inputs::UNITS
        .iter()
        .chain([&pipeline::inputs::TEST_UNIT])
        .filter(|u| {
            ![regions::UNIT, heritage::UNIT, heritage_matches::UNIT, elevation::UNIT, sources::UNIT].contains(*u)
        })
        .map(|u| format!("\"{u}"))
        .collect()
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
            if let Some(k) =
                EXEMPT.iter().position(|(ef, en, anchor, _)| *ef == rel && *en == name && anchored(&lines, i, anchor))
            {
                used.insert(k);
                continue;
            }
            bad.push(format!("{rel}:{} in {name}: {}", i + 1, l.trim()));
        }
    }
    let mut py = Vec::new();
    files(&root.join("dem"), "py", &mut py);
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
            bad.push(format!("{rel}:{} in {name}: {}", i + 1, l.trim()));
        }
    }
    let unused: Vec<String> = EXEMPT
        .iter()
        .enumerate()
        .filter(|(k, _)| !used.contains(k))
        .map(|(_, (f, n, a, _))| format!("{f}: {n}: {a}"))
        .collect();
    (bad, unused)
}

/// Whether an exemption's anchor holds at line `i`: a plain anchor is a part of the line; one
/// starting with `^` is how the item's text begins at the line, its next lines (trimmed) joined, so
/// an exemption names a statement as it was before rustfmt broke it across lines, not any line with
/// the same words.
fn anchored(lines: &[&str], i: usize, anchor: &str) -> bool {
    match anchor.strip_prefix('^') {
        Some(rest) => lines[i..(i + 6).min(lines.len())].iter().map(|l| l.trim()).collect::<String>().starts_with(rest),
        None => lines[i].contains(anchor),
    }
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
    assert!(
        names_a_drop_box("root.join(\"x/inputs/y\")", MARKERS, &[])
            && !names_a_drop_box("\"sources/inputs/x\"", MARKERS, &[])
    );
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
    let out = pipeline::out::Out::unbound(d.path(), &d.path().join("s")).unwrap();
    for p in [
        "inputs/regions/wales.toml",
        "inputs/_gate-test/a.jsonl",
        "translations/fr.jsonl",
        "descriptions/x.jsonl",
        "inputs",
        "./inputs/keys.env",
    ] {
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
    let ok =
        store::spawn::status(store::spawn::command("python3").args(["-I", "-c", "pass"])).is_ok_and(|s| s.success());
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
    let out =
        store::spawn::output(store::spawn::command("python3").arg("-I").arg("-c").arg(code).arg(root().join("dem")))
            .unwrap();
    assert!(
        out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "ok",
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
