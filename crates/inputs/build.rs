// What each gate unit's checks are (docs/inputs.md §4.3, task #1467): the digest of each module of
// src/inputs/ as the app compiles it (build/checks_code.rs), written to $OUT_DIR/checks_code.rs for
// crate::inputs::code_of. It reruns when a file it read changes (each file is named: a module added
// or removed changes src/inputs/mod.rs, a child module its parent's file).
#[path = "build/checks_code.rs"]
mod checks_code;

fn main() {
    let dir = std::path::Path::new("src/inputs");
    let modules = checks_code::modules(dir).unwrap_or_else(|e| panic!("the checks' code digests: {e}"));
    let mut out = String::from("&[\n");
    println!("cargo::rerun-if-changed=build/checks_code.rs");
    println!("cargo::rerun-if-changed={}", dir.join("mod.rs").display());
    for (name, code) in &modules {
        out.push_str(&format!("    ({name:?}, {:?}),\n", code.digest));
        for f in &code.files {
            println!("cargo::rerun-if-changed={}", f.display());
        }
    }
    out.push_str("]\n");
    let path = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("checks_code.rs");
    std::fs::write(&path, out).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}
