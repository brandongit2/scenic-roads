//! The agent's plan: the modules that answer the runtime's questions (`seam::Plan`, agentcore's
//! agent, docs/crates.md §6.3, 6f4) by `theplan`. They name the runtime by its crate
//! (`agentcore::agent::jobs`), which agentcore's unit tests compile them into too.

pub mod build;
pub mod idents;
pub mod steps;
pub mod theplan;
pub mod tiles;
