// The plan's modules are compiled into this crate's unit tests (src/agent/mod.rs), so that the
// runtime's tests run on the real plan; their own tests are pipeline's and run there alone:
// `in_agentcore` leaves them out here (`#[cfg(all(test, not(in_agentcore)))]`).
fn main() {
    println!("cargo::rustc-check-cfg=cfg(in_agentcore)");
    println!("cargo::rustc-cfg=in_agentcore");
}
