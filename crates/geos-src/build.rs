fn main() {
    // Run again only when this script changes: cargo runs a script it compiled anew by itself, and
    // judges that by content (tools/dev/bin/cargo, #1123). `rerun-if-changed=build.rs` was judged by
    // the file's time, so every new worktree ran GEOS's whole CMake build again, and every crate
    // linking it compiled again. A variable named here alone turns off cargo's default (any file of
    // the package); GEOS_SRC_RERUN is set by nothing. (As before, `source/` isn't watched.)
    println!("cargo:rerun-if-env-changed=GEOS_SRC_RERUN");

    let mut libgeos_config = cmake::Config::new("source");
    libgeos_config
        .define("BUILD_BENCHMARKS", "OFF")
        // BUILD_TESTING may need to be set ON for GEOS 3.8.x / 3.9.x due to CMake issues
        .define("BUILD_TESTING", "OFF")
        .define("GEOS_ENABLE_TESTS", "OFF") // GEOS <= 3.7
        .define("BUILD_DOCUMENTATION", "OFF")
        .define("CMAKE_INSTALL_LIBDIR", "lib")
        .define("BUILD_SHARED_LIBS", "OFF") // GEOS >= 3.8
        .define("GEOS_BUILD_STATIC", "ON") // GEOS <= 3.7
        .define("GEOS_BUILD_SHARED", "OFF") // GEOS <= 3.7
        .profile("Release");

    let libgeos = libgeos_config.build();

    println!("cargo:lib=geos_c");
    println!("cargo:lib=geos");

    let search_path = format!("{}/lib", libgeos.display());
    assert!(std::path::Path::new(&search_path).exists());
    println!("cargo:search={}", search_path);
}
