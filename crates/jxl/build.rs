//! libjxl 0.12 (jpegxl-src's copy, built by CMake into OUT_DIR, linked statically) and the shim
//! calling it (shim.c).
fn main() {
    jpegxl_src::build();
    let inc = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("include");
    cc::Build::new().file("shim.c").include(inc).define("JXL_STATIC_DEFINE", None).compile("scenic_jxl");
    println!("cargo:rerun-if-changed=shim.c");
}
