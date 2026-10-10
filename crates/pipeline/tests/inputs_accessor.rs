//! The drop boxes are read only through their accessor (docs/inputs.md §7.1): `pipeline::inputs`
//! (`inputs::open`, the gate's check job and listing), and in Python `dem/inputs.py`'s `arg`. Every
//! function outside tests that names a drop box (`MARKERS`, and each gate unit's name) calls
//! `inputs::`, or is listed in `EXEMPT` with why: the writers (the Regions panel's and `scenic
//! add`'s), the backups, GC's never-swept list, and the readers each input's task moves behind the
//! accessor (#135–#145, removing their exemptions; #146 checks none is left but the writers'). A
//! new reader of a drop box fails here until it's routed through the accessor. At run time the NAS
//! root the steps get (`out::Out::path`) refuses a drop-box path too.

use std::path::{Path, PathBuf};

/// What names a drop box in Rust.
const MARKERS: &[&str] = &["\"inputs", "join(\"inputs\")", "\"translations", "\"descriptions"];

/// What names a drop box in the Python steps.
const PY_MARKERS: &[&str] = &["\"inputs", "/ \"inputs\"", "\"translations", "\"descriptions"];

/// Functions that name a drop box, by file (from the repository's root) and name, with why.
const EXEMPT: &[(&str, &str, &str)] = &[
    // Not reads of a drop box: the `inputs` step's name, the records' names, the owner's tools'
    // writes, the backups and GC's never-swept list, the keys (not an input: docs/inputs.md §2).
    ("crates/pipeline/src/agent/build.rs", "label", "the `inputs` step's name"),
    ("crates/pipeline/src/agent/build.rs", "record", "the `inputs` step's name"),
    ("crates/pipeline/src/agent/mod.rs", "finished", "the `inputs` step's job ids"),
    ("crates/pipeline/src/agent/steps.rs", "w_inputs", "the records' names (sources/inputs/<unit>/index)"),
    ("crates/pipeline/src/agent/backup.rs", "*", "the backups: every drop box and the acceptances, by design (§4.9)"),
    ("crates/pipeline/src/agent/gc.rs", "*", "GC's never-swept list (§4.9)"),
    ("crates/pipeline/src/bin/scenic.rs", "main", "`scenic add`/`remove`: the owner's tool's writes (§2), and the `inputs` command's name"),
    ("crates/pipeline/src/bin/scenic-build.rs", "rail_feeds_step", "inputs/keys.env, not an input (§2): its key names enter keys, never its values"),
    ("crates/pipeline/src/agent/mod.rs", "input_digests", "inputs/keys.env's key names (not an input, §2); the ferries' timetables [#140, #141]"),
    ("crates/pipeline/src/agent/mod.rs", "plan", "inputs/hold-catalog, a control (§2)"),
    // The regions' readers [#135].
    ("crates/pipeline/src/agent/mod.rs", "as_read", "the regions' recipes [#135]"),
    ("crates/pipeline/src/agent/mod.rs", "checklist", "the regions' recipes [#135]"),
    ("crates/pipeline/src/agent/mod.rs", "coverage", "the regions' outline files [#135]"),
    ("crates/pipeline/src/agent/mod.rs", "regions_digest", "the regions' recipes and outline files [#135]"),
    ("crates/pipeline/src/agent/mod.rs", "rekey_records", "the regions' recipes [#135]"),
    ("crates/pipeline/src/agent/mod.rs", "step", "the regions' recipes, for the status [#135]"),
    ("crates/pipeline/src/bin/scenic-build.rs", "catalog_coverage", "the regions' recipes [#135]"),
    ("crates/pipeline/src/bin/scenic-build.rs", "coverage_of", "the regions' recipes, --regions' default [#135]"),
    ("crates/pipeline/src/bin/scenic-build.rs", "p5_check", "the regions' recipes [#135]"),
    ("crates/pipeline/src/bin/scenic-build.rs", "p5_check_terrain", "the regions' recipes [#135]"),
    ("crates/pipeline/src/bin/scenic-build.rs", "pass_and_coverage", "the regions' recipes, --regions' default [#135]"),
    ("crates/pipeline/src/bin/scenic-build.rs", "pois_step", "the regions' recipes, --regions' default [#135]"),
    ("crates/pipeline/src/bin/scenic-build.rs", "rekey_check", "the regions' recipes [#135]"),
    ("crates/pipeline/src/bin/scenic-build.rs", "water_key_check", "the regions' recipes [#135]"),
    ("crates/pipeline/src/bin/terrain.rs", "scan", "the regions' recipes [#135]"),
    ("crates/pipeline/src/bld/task.rs", "spec", "the regions' outline files, for a task [#135]"),
    ("crates/pipeline/src/coverage.rs", "file_rings", "the regions' outline files (poly:) [#135]"),
    ("crates/pipeline/src/offload.rs", "offer", "the regions' recipes [#135]"),
    ("crates/pipeline/src/offload.rs", "run_task", "the regions' recipes [#135]"),
    ("crates/pipeline/src/terrain_task.rs", "spec", "the regions' outline files, for a task [#135]"),
    ("crates/pipeline/src/trees/task.rs", "spec", "the regions' outline files, for a task [#135]"),
    ("crates/server/src/regions.rs", "nas", "the Regions panel: its writes (the owner's tool, §2) and its reads of the recipes [#135]"),
    // The translations' and descriptions' readers [#137].
    ("crates/pipeline/src/namestodo.rs", "run", "the descriptions' folder and the to-do lists [#137]"),
    ("crates/server/src/names_live.rs", "*", "the map server's copy of the translations [#137]"),
    ("crates/server/src/descriptions.rs", "*", "the map server's copy of the descriptions [#137]"),
    // The ferries' timetables [#140, #141].
    ("crates/pipeline/src/ovconv.rs", "ferries_job", "inputs/ferries/freq [#140, #141]"),
];

/// Python functions that name a drop box, by file and name, with why.
const PY_EXEMPT: &[(&str, &str, &str)] = &[
    ("dem/bldfetch.py", "coverage", "inputs/outlines' .poly files for bld-fetch's coverage [#135]"),
    ("dem/railfeeds.py", "keys", "inputs/keys.env, not an input (§2): named in its docstring"),
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

/// The name of the function a line `i` of `lines` is in (the last `fn <name>` / `def <name>` above
/// it), and that function's text (to the next one at its indentation or less).
fn enclosing(lines: &[&str], i: usize, kw: &str) -> Option<(String, String)> {
    let starts = |l: &str| {
        let t = l.trim_start();
        let t = t.strip_prefix("pub(crate) ").or_else(|| t.strip_prefix("pub(super) ")).or_else(|| t.strip_prefix("pub ")).unwrap_or(t);
        let t = t.strip_prefix("async ").unwrap_or(t);
        t.strip_prefix(kw).map(|r| r.split(|c: char| !(c.is_alphanumeric() || c == '_')).next().unwrap_or("").to_string())
    };
    let indent = |l: &str| l.len() - l.trim_start().len();
    let (at, name) = (0..=i).rev().find_map(|k| starts(lines[k]).map(|n| (k, n)))?;
    let end = (at + 1..lines.len()).find(|&k| starts(lines[k]).is_some() && indent(lines[k]) <= indent(lines[at]) && !lines[k].trim().is_empty()).unwrap_or(lines.len());
    Some((name, lines[at..end].join("\n")))
}

/// Each gate unit's name as code would write it in a path.
fn unit_markers() -> Vec<String> {
    pipeline::inputs::UNITS.iter().chain([&pipeline::inputs::TEST_UNIT]).map(|u| format!("\"{u}")).collect()
}

#[test]
fn every_drop_box_read_goes_through_the_accessor() {
    let root = root();
    let units = unit_markers();
    let mut bad = Vec::new();
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
        // (Tests aside: they make their own drop boxes.)
        let end = (1..lines.len()).find(|&k| ["mod tests", "pub mod tests", "pub(crate) mod tests", "mod pool_tests"].iter().any(|m| lines[k].trim_start().starts_with(m)) && lines[k - 1].contains("cfg(test)")).unwrap_or(lines.len());
        for i in 0..end {
            let l = lines[i];
            let t = l.trim_start();
            if t.starts_with("//") || !(MARKERS.iter().any(|m| l.contains(m)) || units.iter().any(|m| l.contains(m.as_str()))) {
                continue;
            }
            let Some((name, body)) = enclosing(&lines, i, "fn ") else { continue };
            if body.contains("inputs::") || EXEMPT.iter().any(|(ef, en, _)| *ef == rel && (*en == "*" || *en == name)) {
                continue;
            }
            bad.push(format!("{rel}:{} in fn {name}: {}", i + 1, t));
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
            if l.trim_start().starts_with('#') || !(PY_MARKERS.iter().any(|m| l.contains(m)) || units.iter().any(|m| l.contains(m.as_str()))) {
                continue;
            }
            let Some((name, body)) = enclosing(&lines, i, "def ") else { continue };
            if body.contains("inputs.arg(") || PY_EXEMPT.iter().any(|(ef, en, _)| *ef == rel && (*en == "*" || *en == name)) {
                continue;
            }
            bad.push(format!("{rel}:{} in def {name}: {}", i + 1, l.trim()));
        }
    }
    assert!(bad.is_empty(), "drop boxes named outside their accessor (pipeline::inputs, dem/inputs.py); route the read through it, or list the function in EXEMPT with why:\n{}", bad.join("\n"));
}

/// Every exemption still names a function that's there (a stale one would let a new reader by).
#[test]
fn every_exemption_names_a_function_there() {
    let root = root();
    for (f, name, _) in EXEMPT {
        let text = std::fs::read_to_string(root.join(f)).unwrap_or_else(|_| panic!("{f} isn't there"));
        assert!(*name == "*" || text.contains(&format!("fn {name}(")) || text.contains(&format!("fn {name}<")) || text.contains(&format!("mod {name}")), "{f}: no fn {name}");
    }
    for (f, name, _) in PY_EXEMPT {
        let text = std::fs::read_to_string(root.join(f)).unwrap_or_else(|_| panic!("{f} isn't there"));
        assert!(*name == "*" || text.contains(&format!("def {name}(")), "{f}: no def {name}");
    }
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
