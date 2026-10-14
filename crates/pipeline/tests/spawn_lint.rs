//! Every child in the workspace is started, and every listener made and connection taken, by
//! store::spawn (docs/pool.md §7.5): clippy's disallowed-methods (clippy.toml) over the whole
//! workspace, every target, finds none started another way. Its own target folder, so it waits on
//! no other build.

#[test]
fn every_child_is_started_by_spawn() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut c = std::process::Command::new(cargo);
    c.current_dir(&root)
        .env("CARGO_TARGET_DIR", root.join("target/spawn-lint"))
        .args(["clippy", "--workspace", "--all-targets", "--quiet", "--message-format=short", "--", "-A", "clippy::all", "-D", "clippy::disallowed_methods"]);
    let out = store::spawn::output(&mut c).expect("cargo clippy");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "a child, listener or connection not by store::spawn:\n{said}");
}
