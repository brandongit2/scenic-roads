//! The declared elevation datasets (docs/inputs.md §6.8): their declarations and what the input
//! check measures (`declared`), the tile encodings, the one generic sampler over them in their
//! order, blended at their edges (`generic`: what the terrain's base reads, crate::terrain_base),
//! the heights' conversions to EGM2008 (`geoid`), the projections, the roads' spacing (`spacing`)
//! and a stored dataset's store (`store_fetch`). No source is named here. The roads take their
//! heights from the finished terrain (crate::road_terrain), not from these.

pub mod declared;
pub mod encoding;
pub mod generic;
pub mod geoid;
pub mod proj;
pub mod sample;
pub mod spacing;
pub mod store_fetch;

#[cfg(test)]
mod tests;
