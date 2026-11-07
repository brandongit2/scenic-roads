//! The agent: its runtime is agentcore's (docs/crates.md §6.3, 6f4), at this path; the plan's
//! modules are this crate's, and answer the runtime's questions (`seam::Plan`) by `theplan`.

pub use agentcore::agent::*;

pub mod build;
pub mod idents;
pub mod steps;
pub mod theplan;
pub mod tiles;
