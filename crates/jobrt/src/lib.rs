//! A job's runtime (docs/crates.md, task #916 step 3): what a build step's job and its agent share
//! below the steps: the pause and stop protocol (control), the phases' timings, the process and
//! system calls (sys), its spans (spans), the budgets and their proxy, the resume log, a job's progress lines and its
//! room target, the pool's settings, a job run's id (jobid); and the app's updater (appupdate:
//! every Mac's agent and the map server run it, step 7a). Pipeline re-exports these modules at their old paths.

pub mod appupdate;
pub mod budget;
pub mod control;
pub mod jobid;
pub mod jobs;
pub mod keys;
pub mod phase_names;
pub mod pyenv;
pub mod resume_log;
pub mod room;
pub mod settings;
pub mod spans;
pub mod sys;
pub mod timings;

/// The map server's settings profiles, a file in the app's folder on the server's Mac
/// (crates/server/src/profiles.rs), which the lead's daily backup job copies (agentcore::agent::backup).
pub const PROFILES: &str = "profiles.json";
