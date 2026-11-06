pub mod agent;
pub mod answers;
pub mod appupdate;
pub mod areafiles;
pub mod bld;
pub mod buildtiles;
pub mod candidates;
pub mod cli;
pub mod coord;
pub mod dem;
pub mod depth;
pub mod elev;
pub mod ferries;
pub mod gtfs;
pub mod heritage;
pub mod inputs;
pub mod lakefill;
pub mod landcover;
pub mod layers;
pub mod leaftype;
pub mod markconv;
pub mod marksjob;
pub mod menu;
pub mod namestodo;
pub mod needs;
pub mod net;
pub mod offload;
pub mod osmpass;
pub mod out;
pub mod ovconv;
pub mod paces;
pub mod peaks;
pub mod polar;
pub mod pool;
pub mod pubtimes;
pub mod pyramids;
pub mod rail;
pub mod railfeeds;
pub mod reach;
pub mod road_terrain;
pub mod rules;
pub mod slope_pack;
pub mod stage;
pub mod summits;
pub mod terrain_base;
pub mod terrain_deep;
pub mod terrain_pack;
pub mod terrain_resume;
pub mod terrain_task;
pub mod terrain_zoom;
pub mod timetables;
pub mod treepacks;
pub mod trees;
pub mod unit;
pub mod uses;
pub mod water;

// Moved to their own crates (task #916, docs/crates.md), kept at their old paths for the modules above
// the steps and the binaries (the step modules name their crates: step 6b). Step 2's:
pub use codecs::{
    areaflags, basepack, basepart, bytes_bar, count_bar, extents, hipack, legacy, marks, morton, shade_codec, summary,
    terrain_codec, terrain_water, tiling, trailends, watercov, whole, ProgressRead,
};

// Step 3's:
pub use jobrt::{budget, control, phase_names, pyenv, resume_log, sys, timings};

// Step 4's (out is pipeline's own: its `open` gives buildstore's `Out` the steps' write sets):
pub use buildstore::{costin, coverage, fetch, geotiff, handoff, localread, outlines, rawpack};

// Step 5a's:
pub use landmarks::{
    areadetails, filterprops, heritage_layers, heritagedetails, heritagewd, interest, items, labels, pageviews,
    whsshapes,
};

// Step 5b's:
pub use mapview::{buildings, scache, terr, view};

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
mod grant_tests;
