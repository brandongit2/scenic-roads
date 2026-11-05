//! The landmarks' Wikidata and heritage steps (docs/crates.md, task #916 step 5a): the asks of
//! Wikidata and Wikipedia (wikiask), the heritage sites' Wikidata chain (heritagewd,
//! heritage_layers, heritagedetails, filterprops), the landmarks' interest (interest), the items
//! and their pageviews (items, pageviews), the labels (labels), the areas' details (areadetails) and
//! the World Heritage Sites' shapes (whsshapes). Pipeline re-exports these modules at their old
//! paths.

pub mod areadetails;
pub mod filterprops;
pub mod heritage_layers;
pub mod heritagedetails;
pub mod heritagewd;
pub mod interest;
pub mod items;
pub mod labels;
pub mod pageviews;
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
