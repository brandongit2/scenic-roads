//! The declared inputs and the steps that read them first (docs/crates.md, task #916 step 6d): the
//! inputs folders' gate and what it takes in (inputs), the elevation datasets and their zoom rule
//! (dem), the map's rules and credits (rules), the OSM pass (osmpass) and its planet (planet), the region downloads' files
//! (areafiles), the rail feeds and their counts (rail), the lake fill (lakefill) and the water
//! (water). Pipeline re-exports them at their old paths.

pub mod areafiles;
pub mod dem;
pub mod inputs;
pub mod lakefill;
pub mod osmpass;
pub mod planet;
pub mod rail;
pub mod rules;
pub mod water;

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
