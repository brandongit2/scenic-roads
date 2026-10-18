//! The in-use share (docs/pool.md §7.1): the cores and memory a Mac's agent keeps free of its jobs
//! while its owner uses the Mac. Set by `scenic in-use` in the agent's folder, which the agent reads
//! each loop (as the disk room target, crate::agent::room); either part not set takes its default.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The share as set: its cores and memory (MB) where said, who set it and when (unix seconds).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Share {
    pub cores: Option<u32>,
    pub mem_mb: Option<u64>,
    pub by: String,
    pub at: u64,
}

/// The share's file, in the agent's folder.
pub const FILE: &str = "in-use-share.json";

/// The cores kept by default: two.
pub const DEFAULT_CORES: u32 = 2;

/// The memory kept by default: five eighths of the Mac's (a member spares its jobs three eighths
/// while its owner is about, crate::agent's `helper_memory`).
pub fn default_mem_mb(total_mb: u64) -> u64 {
    total_mb * 5 / 8
}

/// The share as set in `home`, if any.
pub fn read(home: &Path) -> Option<Share> {
    std::fs::read(home.join(FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

/// Sets the share in `home` (both parts None: cleared, the defaults again).
pub fn set(home: &Path, cores: Option<u32>, mem_mb: Option<u64>, by: &str) -> Result<Option<Share>> {
    let p = home.join(FILE);
    if cores.is_none() && mem_mb.is_none() {
        match std::fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        return Ok(None);
    }
    std::fs::create_dir_all(home)?;
    let s = Share { cores, mem_mb, by: by.to_string(), at: super::jobs::now_s() };
    crate::whole::write(&p, &serde_json::to_vec(&s)?)?;
    Ok(Some(s))
}

/// The cores kept of a Mac's `cores` while its owner uses it: the share's, else the default; at
/// least one left to the jobs.
pub fn cores(s: Option<&Share>, cores: u32) -> u32 {
    s.and_then(|s| s.cores).unwrap_or(DEFAULT_CORES).min(cores.saturating_sub(1))
}

/// The memory (MB) kept of a Mac's `total_mb` while its owner uses it: the share's, else the default.
pub fn mem_mb(s: Option<&Share>, total_mb: u64) -> u64 {
    s.and_then(|s| s.mem_mb).unwrap_or_else(|| default_mem_mb(total_mb)).min(total_mb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_share_is_set_read_defaulted_and_cleared() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(read(d.path()), None);
        assert_eq!((cores(None, 8), mem_mb(None, 16_384)), (2, 10_240));
        assert_eq!(cores(None, 1), 0);
        let s = set(d.path(), Some(4), None, "a test").unwrap();
        assert_eq!(read(d.path()), s);
        let s = read(d.path());
        assert_eq!((cores(s.as_ref(), 8), mem_mb(s.as_ref(), 16_384)), (4, 10_240));
        set(d.path(), None, Some(4096), "a test").unwrap();
        let s = read(d.path());
        assert_eq!((cores(s.as_ref(), 8), mem_mb(s.as_ref(), 16_384)), (2, 4096));
        assert_eq!(set(d.path(), None, None, "a test").unwrap(), None);
        assert_eq!(read(d.path()), None);
    }
}
