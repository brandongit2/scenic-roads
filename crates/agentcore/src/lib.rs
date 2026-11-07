//! The agent's runtime (task #916, docs/crates.md §6.3, 6f4): the agent and its planner, the
//! coordinator, the pool, the forecast, the command lines. The plan (pipeline's agent::build,
//! steps, idents, tiles) sits above it and is asked through `agent::seam::Plan`, which the binaries
//! hand in.

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

// The crates below, at the paths the runtime names them by (as pipeline's root did).
pub use buildstore::{costin, coverage, fetch, handoff, localread, outlines, rawpack};
pub use codecs::{extents, legacy, whole};
pub use coordwire::net;
pub use jobrt::{budget, control, phase_names, pyenv, resume_log, sys, timings};

// (`::inputs`: the crate, not its module of the same name.)
pub use ::inputs::{areafiles, dem, inputs, lakefill, osmpass, rail};

// The plan's modules (agent/mod.rs) and the families its code and the runtime's tests name, in the
// unit tests alone, at pipeline's paths.
/// Where the plan's steps put their outputs, at pipeline's path (its `out`, but `open`).
#[cfg(test)]
pub mod out {
    pub use buildstore::out::*;
    pub use buildstore::records::read_manifest;
}
#[cfg(test)]
pub use ::inputs::{rules, water};
#[cfg(test)]
pub use codecs::{hipack, marks, shade_codec, terrain_codec, terrain_water};
#[cfg(test)]
pub use landmarks::{heritage, markconv, namestodo, ovconv};
#[cfg(test)]
pub use roads::{reach, stage, timetables, unit};
#[cfg(test)]
pub use terrain::{
    depth, layers, polar, pyramids, slope_pack, terrain_deep, terrain_pack, terrain_resume, terrain_task, terrain_zoom,
};
// (`::trees` and `::bld`: the crates, not their modules of the same name.)
#[cfg(test)]
pub use ::bld::{bld, buildtiles};
#[cfg(test)]
pub use ::trees::{treepacks, trees};

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
