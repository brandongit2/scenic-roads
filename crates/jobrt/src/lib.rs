//! A job's runtime (docs/crates.md, task #916 step 3): what a build step's job and its agent share
//! below the steps: the pause and stop protocol (control), the phases' timings, the process and
//! system calls (sys), the budgets and their proxy, the resume log, a job's progress lines and its
//! room target, the pool's settings. Pipeline re-exports these modules at their old paths.

pub mod budget;
pub mod control;
pub mod jobs;
pub mod phase_names;
pub mod pyenv;
pub mod resume_log;
pub mod room;
pub mod settings;
pub mod sys;
pub mod timings;
