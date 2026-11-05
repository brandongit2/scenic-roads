pub mod agent;
pub mod answers;
pub mod appupdate;
pub mod areafiles;
pub mod bld;
pub mod buildings;
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
pub mod scache;
pub mod slope_pack;
pub mod stage;
pub mod summits;
pub mod terr;
pub mod terrain_base;
pub mod terrain_deep;
pub mod terrain_lakes;
pub mod terrain_pack;
pub mod terrain_resume;
pub mod terrain_task;
pub mod terrain_zoom;
pub mod timetables;
pub mod treepacks;
pub mod trees;
pub mod unit;
pub mod uses;
pub mod view;
pub mod water;

// Moved to their own crate (task #916 step 1, docs/crates.md), kept at their old paths.
pub use pyport::{pyjson, pynumpy, pytext, tpsimplify};

// Moved to their own crate (task #916 step 2, docs/crates.md), kept at their old paths.
pub use codecs::{
    areaflags, basepack, basepart, bytes_bar, chain, count_bar, extents, hipack, legacy, marks, morton, shade_codec,
    stations, summary, terrain_codec, terrain_water, tiling, trailends, vtgen, watercov, webp, whole, ProgressRead,
};

// Moved to their own crate (task #916 step 3, docs/crates.md), kept at their old paths.
pub use jobrt::{budget, control, phase_names, pyenv, resume_log, sys, timings};

// Moved to their own crate (task #916 step 4, docs/crates.md), kept at their old paths. (out is
// pipeline's own: its `Out` gives buildstore's the records and write sets from above.)
pub use buildstore::{costin, coverage, fetch, geotiff, handoff, localread, outlines, rawpack};

// Moved to their own crate (task #916 step 5a, docs/crates.md), kept at their old paths.
pub use landmarks::{
    areadetails, filterprops, heritage_layers, heritagedetails, heritagewd, interest, items, labels, pageviews,
    whsshapes, wikiask,
};

/// Pipeline's unit tests' process is offline from its start (buildstore::fetch::go_offline), before
/// any test runs, as it was while fetch was pipeline's: run by the loader as the test binary starts
/// (an initializer section's entry). Only in `cfg(test)`, this crate's own unit tests: its
/// binaries, examples and integration tests link the library without it and reach the network
/// (tests/test_mode.rs). A test-time behaviour never rides on a feature (buildstore's `testkit`).
#[cfg(test)]
#[used]
#[cfg_attr(target_os = "macos", link_section = "__DATA,__mod_init_func")]
#[cfg_attr(not(target_os = "macos"), link_section = ".init_array")]
static TEST_START: extern "C" fn() = {
    extern "C" fn start() {
        buildstore::fetch::go_offline();
    }
    start
};

#[cfg(test)]
mod test_start;

#[cfg(test)]
mod grant_tests;
