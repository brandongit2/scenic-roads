// The plan's modules are compiled into this crate's unit tests (src/agent/mod.rs), so that the
// runtime's tests run on the real plan; their own tests are pipeline's and run there alone:
// `in_agentcore` leaves them out here (`#[cfg(all(test, not(in_agentcore)))]`).
// It reads nothing but itself: without the line below cargo would run it again after any edit in
// the crate, ahead of the crate's own compile.
fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rustc-check-cfg=cfg(in_agentcore)");
    println!("cargo::rustc-cfg=in_agentcore");
}
