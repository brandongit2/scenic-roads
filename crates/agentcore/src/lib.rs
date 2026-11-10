//! The agent's runtime (task #916, docs/crates.md §6.3, 6f4): the agent and its planner, the
//! coordinator, the pool, the forecast, the command lines. The plan (pipeline's agent::build,
//! steps, idents, tiles) sits above it and is asked through `agent::seam::Plan`, which the binaries
//! hand in.

// The plan's files are compiled into this crate's unit tests (agent/mod.rs's `#[path]` modules) and
// name the runtime as pipeline does, by this crate's name (`agentcore::coord`): here that's this
// crate itself.
extern crate self as agentcore;

pub mod agent;
pub mod answers;
pub mod cli;
pub mod coord;
pub mod menu;
pub mod needs;
pub mod paces;
pub mod pool;
pub mod pubtimes;
pub mod uses;

/// The runtime's unit tests' process is offline from its start (buildstore::fetch::go_offline), and
/// an `Out`'s waits are a test's (buildstore::out::go_testing), before any test runs: run by the
/// loader as the test binary starts (an initializer section's entry), as pipeline's root does for
/// its own. Only in `cfg(test)`, this crate's own unit tests: every program linking the library
/// runs without it (pipeline's tests/test_mode.rs).
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
