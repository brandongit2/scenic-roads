//! The scenic metrics' map-side parts (docs/crates.md, task #916 step 5b): the far-field viewshed
//! and landscape metrics (view), the roadside buildings (buildings), the terrain lookups from the
//! Terrarium archive (terr) and the caches that let the scenic steps redo only what is new
//! (scache). Pipeline re-exports these modules at their old paths.

pub mod buildings;
pub mod scache;
pub mod terr;
pub mod view;
