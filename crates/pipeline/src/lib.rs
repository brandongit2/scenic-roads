pub mod agent;
pub mod offload;
pub mod out;

// The runtime (step 6f4's: crates/agentcore), at its old paths; the plan's modules in `agent`.
pub use agentcore::{answers, cli, coord, menu, needs, pool, pubtimes};

// Moved to their own crates (task #916, docs/crates.md), kept at their old paths for the modules above
// the steps and the binaries (the step modules name their crates: step 6b). Step 2's:
pub use codecs::{
    areaflags, basepack, basepart, bytes_bar, count_bar, extents, hipack, legacy, marks, morton, shade_codec, summary,
    terrain_codec, terrain_water, tiling, trailends, watercov, whole, ProgressRead,
};

// Step 3's:
pub use jobrt::{budget, control, pyenv, resume_log, sys, timings};
// Step 7a's (what the map server names, moved below the runtime):
pub use coordwire::net;
pub use jobrt::appupdate;

// Step 4's (out is pipeline's own: its `open` gives buildstore's `Out` the steps' write sets):
pub use buildstore::{costin, coverage, fetch, geotiff, handoff, localread, outlines, rawpack};

// Step 5a's:
pub use landmarks::{
    areadetails, filterprops, heritage_layers, heritagedetails, heritagewd, interest, items, labels, pageviews,
    whsshapes,
};

// Step 5b's:
pub use mapview::{buildings, scache, terr, view};

// Step 6d's (`::inputs`: the crate, not its module of the same name):
pub use ::inputs::{areafiles, dem, inputs, lakefill, osmpass, rail, rules, water};

// Step 6e's:
pub use terrain::{
    depth, layers, polar, pyramids, road_terrain, slope_pack, terrain_base, terrain_deep, terrain_pack, terrain_resume,
    terrain_task, terrain_zoom,
};
// (`::trees`: the crate, not its module of the same name.)
pub use ::trees::{landcover, treepacks, trees};
pub use landmarks::{gtfs, heritage, markconv, marksjob, namestodo, ovconv, railfeeds};
pub use roads::{candidates, elev, peaks, reach, stage, summits, timetables, unit};
// (`::bld`: the crate, not its module of the same name.)
pub use ::bld::{bld, buildtiles};

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
