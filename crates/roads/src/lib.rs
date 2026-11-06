//! The roads' steps (docs/crates.md, task #916 step 6e): a z6 tile's unit, its staged inputs and
//! its runs (unit, stage), the roads' reach (reach), the points of interest (candidates), the
//! roads' elevations (elev), the peaks and summits (peaks, summits) and the timetables' to-do
//! (timetables). Pipeline re-exports them at their old paths.

pub mod candidates;
pub mod elev;
pub mod peaks;
pub mod reach;
pub mod stage;
pub mod summits;
pub mod timetables;
pub mod unit;

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
