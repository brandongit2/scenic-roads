//! The buildings (docs/crates.md, task #916 step 6e): the 3D buildings from Overture's release and
//! GHSL's heights, fetched onto the NAS, normalized per z6 tile and made into tiles and their field,
//! a z8 tile at a time as tasks for other Macs (bld), and the roadside buildings a unit's roads read
//! (buildtiles). Pipeline re-exports them at their old paths.

pub mod bld;
pub mod buildtiles;

/// These unit tests' process is offline from its start (buildstore::fetch::go_offline), and an
/// `Out`'s waits and a disk's free space are a test's (buildstore::out::go_testing), as they were
/// in pipeline's (its lib.rs `TEST_START`): only in `cfg(test)`.
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
