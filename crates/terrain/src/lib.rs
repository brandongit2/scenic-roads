//! The terrain and slope steps (docs/crates.md, task #916 step 6e): the terrain's pool and sampling
//! (terrain_base), its packs, deep tiles and their zoom rule (terrain_pack, terrain_deep,
//! terrain_zoom, terrain_resume), its z8 subtrees as tasks for other Macs (terrain_task), the
//! slopes (slope_pack), the water's depth (depth), the polar caps (polar), the elevation pyramids
//! (pyramids), the terrain a z6 tile's roads read (road_terrain) and the layers' packs and names
//! (layers). Pipeline re-exports them at their old paths.

pub mod available;
pub mod depth;
pub mod layers;
pub mod polar;
pub mod pyramids;
pub mod road_terrain;
pub mod slope_pack;
pub mod terrain_base;
pub mod terrain_deep;
pub mod terrain_pack;
pub mod terrain_resume;
pub mod terrain_task;
pub mod terrain_zoom;

/// These unit tests' process is offline from its start (buildstore::fetch::go_offline), and an
/// `Out`'s waits are a test's (buildstore::out::go_testing), as they were in pipeline's (its lib.rs
/// `TEST_START`): only in `cfg(test)`.
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
