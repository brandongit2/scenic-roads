//! A task's side that its job and its worker share (docs/coordinator.md §2): the offer a job makes
//! (`Offer`), the outputs a worker hands back (`Output`), the paths a worker may name (`safe`), a
//! task's files cloned (`clone`), and whether a worker's pace beats the job's own run (`beats`,
//! `MARGIN`). The coordinator's own side (the tasks it holds, their leases and paces) is pipeline's
//! coord::task, which re-exports `Offer`, `Output` and `beats`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Output {
    pub path: String,
    pub size: u64,
}

/// A task as a job offers it (`POST /task/offer`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Offer {
    pub owner: u32,
    pub kind: String,
    pub spec: serde_json::Value,
    pub root: PathBuf,
    pub inputs: BTreeMap<String, u64>,
    pub mem_mb: u64,
    /// Offered ahead (pipeline's coord::task::Task::ahead): said only by a job told to (jobrt::jobs::AHEAD_ENV).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ahead: Option<f64>,
}

/// The margin a worker's pace is taken with: it's waited on only if its time, a quarter more,
/// comes before the job's own run would end.
pub const MARGIN: f64 = 1.25;

/// Whether a worker at `pace` is faster than the job's own run, with the margin.
pub fn beats(pace: f64) -> bool {
    pace * MARGIN < 1.0
}

/// A path a worker names (an input it fetches, an output it uploads), when it stays inside the
/// folder: relative, no `..`, nothing odd.
pub fn safe(path: &str) -> Option<&Path> {
    let p = Path::new(path);
    let ok = !path.is_empty()
        && path.len() < 1024
        && p.components()
            .all(|c| matches!(c, Component::Normal(s) if !s.to_string_lossy().starts_with('.') || s.len() > 2));
    ok.then_some(p).filter(|p| !p.to_string_lossy().contains('\0'))
}

/// A clone of `src` at `dst` (store::sys::copy_data: copy-on-write where the volume can, else a
/// copy); a link stays a link (to the same file, served from there: a z6 tile's roadside buildings
/// are links to the NAS's).
pub fn clone(src: &Path, dst: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    if std::fs::symlink_metadata(src)?.file_type().is_symlink() {
        let to = std::fs::read_link(src)?;
        let to = if to.is_relative() { src.parent().unwrap_or(Path::new(".")).join(to) } else { to };
        return std::os::unix::fs::symlink(to, dst);
    }
    store::sys::copy_data(src, dst).map(|_| ())
}
