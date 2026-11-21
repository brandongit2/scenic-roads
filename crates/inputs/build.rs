// What each gate unit's checks are (docs/inputs.md §4.3, task #1467): the digest of the code each
// module of src/inputs/ compiles to, with all it names in this crate and the crates it reaches by
// path dependencies (build/checks_code.rs), written to $OUT_DIR/checks_code.rs for
// crate::inputs::code_of. It reruns when a file it read changes: every source and manifest of
// those crates (a module added or removed changes its parent's file).
// And the held digests that no longer match their code (#1578, checks_code::stale_codes): each
// printed as a warning saying the value to set, on any Mac's build, and written to
// $OUT_DIR/stale_codes, which tools/app/guards.sh (check.sh's guards part, publish's) fails on.
#[path = "build/checks_code.rs"]
mod checks_code;

fn main() {
    let w = checks_code::World::read(std::path::Path::new(".")).unwrap_or_else(|e| panic!("the checks' code: {e}"));
    let mut out = String::from("&[\n");
    println!("cargo::rerun-if-changed=build/checks_code.rs");
    for f in &w.files {
        println!("cargo::rerun-if-changed={}", f.display());
    }
    for (name, digest) in checks_code::codes(&w).unwrap_or_else(|e| panic!("the checks' code: {e}")) {
        out.push_str(&format!("    ({name:?}, {digest:?}),\n"));
    }
    out.push_str("]\n");
    let dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let path = dir.join("checks_code.rs");
    std::fs::write(&path, out).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    let read = |p: &str| std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{p}: {e}"));
    let (regions, elevation) = (read("src/inputs/regions/mod.rs"), read("src/inputs/elevation.rs"));
    // (A held code's helper failing an assert of its own, a method renamed, is its test's to say:
    // the build goes on, and the guard fails naming it.)
    let stale = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| checks_code::stale_codes(&w, &regions, &elevation)))
        .unwrap_or_else(|_| {
            vec!["the held digests can't be computed (build/checks_code.rs panicked: its test, checks_code_tests, says where)".into()]
        });
    for s in &stale {
        println!("cargo::warning={s}");
    }
    let path = dir.join("stale_codes");
    std::fs::write(&path, stale.iter().map(|s| format!("{s}\n")).collect::<String>())
        .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}
