pub mod agent;
pub mod offload;
pub mod out;

// The runtime's modules the plan names at the crate's root (`crate::coord`, …): the plan's files
// are compiled into agentcore's unit tests too (agentcore's agent/mod.rs), where these are
// agentcore's own, so the plan can't name them by the agentcore crate. Everything else names the
// crate that owns it (task #916, docs/crates.md §2).
pub use agentcore::{answers, coord, needs, pool};

/// Pipeline's unit tests' process is offline from its start (buildstore::fetch::go_offline), and an
/// `Out`'s waits are a test's (buildstore::out::go_testing), before any test runs, as it was while
/// fetch was pipeline's: run by the loader as the test binary starts (an initializer section's
/// entry). Only in `cfg(test)`, this crate's own unit tests: its
/// binaries, examples and integration tests link the library without it and reach the network
/// (tests/test_mode.rs). A test-time behaviour never rides on a feature (buildstore's `testkit`).
#[cfg(test)]
#[used]
#[cfg_attr(target_os = "macos", link_section = "__DATA,__mod_init_func")]
#[cfg_attr(not(target_os = "macos"), link_section = ".init_array")]
static TEST_START: extern "C" fn() = {
    extern "C" fn start() {
        buildstore::fetch::go_offline();
        buildstore::out::go_testing();
    }
    start
};

#[cfg(test)]
mod test_start;

#[cfg(test)]
mod inputs_tests;

#[cfg(test)]
mod terrain_tests;

#[cfg(test)]
mod trees_tests;

#[cfg(test)]
mod roads_tests;

#[cfg(test)]
mod names_tests;

#[cfg(test)]
mod bld_tests;
