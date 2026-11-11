// The plan's modules are compiled into this crate's unit tests (src/agent/mod.rs), so that the
// runtime's tests run on the real plan; their own tests are pipeline's and run there alone:
// `in_agentcore` leaves them out here (`#[cfg(all(test, not(in_agentcore)))]`).
// It reads nothing but itself: without the line below cargo would run it again after any edit in
// the crate, ahead of the crate's own compile. A variable set by nothing, not `rerun-if-changed=
// build.rs`: cargo runs a script it compiled anew by itself, judged by content, where a path is
// judged by its time, and every new worktree ran this again and compiled agentcore anew (#1123).
fn main() {
    println!("cargo::rerun-if-env-changed=SCENIC_AGENTCORE_BUILD_RERUN");
    println!("cargo::rustc-check-cfg=cfg(in_agentcore)");
    println!("cargo::rustc-cfg=in_agentcore");
}
