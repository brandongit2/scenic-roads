// What each gate unit's checks are (docs/inputs.md §4.3, task #1467): the digest of the code each
// module of src/inputs/ compiles to, with all it names in this crate and the crates it reaches by
// path dependencies (build/checks_code.rs), written to $OUT_DIR/checks_code.rs for
// crate::inputs::code_of. It reruns when a file it read changes: every source and manifest of
// those crates (a module added or removed changes its parent's file).
#[path = "build/checks_code.rs"]
mod checks_code;

fn main() {
    let w = checks_code::World::read(std::path::Path::new(".")).unwrap_or_else(|e| panic!("the checks' code: {e}"));
    let mut out = String::from("&[\n");
    println!("cargo::rerun-if-changed=build/checks_code.rs");
    for f in &w.files {
        println!("cargo::rerun-if-changed={}", f.display());
    }
    for (name, digest) in checks_code::codes(&w) {
        out.push_str(&format!("    ({name:?}, {digest:?}),\n"));
    }
    out.push_str("]\n");
    let path = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("checks_code.rs");
    std::fs::write(&path, out).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}
