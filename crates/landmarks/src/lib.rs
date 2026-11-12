//! The names family: the landmarks', places' and transit's steps (docs/crates.md, task #916 steps
//! 5a and 6e): the asks of Wikidata and Wikipedia (wikiask), the heritage sites (heritage) and
//! their Wikidata chain (heritagewd, heritage_layers, heritagedetails, filterprops), the landmarks'
//! interest (interest), the items and their pageviews (items, pageviews), the labels (labels), the
//! polar caps' points past 85.05° (polar), the areas' details (areadetails), the World Heritage Sites' shapes (whsshapes), the marks and the
//! overlays (markconv, marksjob, ovconv), the timetables (gtfs, railfeeds, ferries) and the
//! descriptions to write (namestodo). Pipeline re-exports those its binaries and agent name at
//! their old paths (all but wikiask).

pub mod areadetails;
pub mod ferries;
pub mod filterprops;
pub mod gtfs;
pub mod heritage;
pub mod heritage_layers;
pub mod heritagedetails;
pub mod heritagewd;
pub mod interest;
pub mod items;
pub mod labels;
pub mod markconv;
pub mod marksjob;
pub mod namestodo;
pub mod ovconv;
pub mod pageviews;
pub mod polar;
pub mod railfeeds;
pub mod whsshapes;
pub mod wikiask;

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
