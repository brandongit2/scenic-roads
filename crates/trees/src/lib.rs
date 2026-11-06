//! The tree cover step (docs/crates.md, task #916 step 6e): the tree cover layers' tiles of a z6
//! tile's blocks and a z3 tile's assembly, a z6 tile's rows as tasks for other Macs (trees), the
//! jobs packing them (treepacks), the leaf-type squares made from their sources (leaftype) and the
//! land cover on the analysis grid (landcover). Pipeline re-exports them at their old paths.

pub mod landcover;
pub mod leaftype;
pub mod treepacks;
pub mod trees;

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
