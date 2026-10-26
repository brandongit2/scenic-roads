//! The in-use share (docs/pool.md §7.1): the cores a Mac's agent keeps free of its jobs for its
//! owner while they use the Mac (used within five minutes). Set by `scenic in-use` in the agent's
//! folder, which the agent reads each loop (as the disk room target, crate::agent::room); not set,
//! the default. (The memory the build's jobs get is crate::agent::buildmem's, whether the owner
//! uses the Mac or not.)

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The share as set: its cores where said, who set it and when (unix seconds). (A file from before
/// the build's memory (buildmem) may say `mem_mb` too: ignored.)
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Share {
    pub cores: Option<u32>,
    pub by: String,
    pub at: u64,
}

/// The share's file, in the agent's folder.
pub const FILE: &str = "in-use-share.json";

/// The cores kept by default: two.
pub const DEFAULT_CORES: u32 = 2;

/// The share as set in `home`, if any (one that can't be read: none, the defaults).
pub fn read(home: &Path) -> Option<Share> {
    read_checked(home).ok().flatten()
}

/// The share as set in `home`: None when there's none; an error when its file can't be read or
/// parsed.
pub fn read_checked(home: &Path) -> Result<Option<Share>> {
    let p = home.join(FILE);
    match std::fs::read(&p) {
        Ok(b) => {
            Ok(Some(serde_json::from_slice(&b).map_err(|e| anyhow::anyhow!("{} can't be read: {e}", p.display()))?))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::anyhow!("{} can't be read: {e}", p.display())),
    }
}

/// Sets the share in `home` (None: cleared, the default again).
pub fn set(home: &Path, cores: Option<u32>, by: &str) -> Result<Option<Share>> {
    let p = home.join(FILE);
    if cores.is_none() {
        match std::fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        return Ok(None);
    }
    std::fs::create_dir_all(home)?;
    let s = Share { cores, by: by.to_string(), at: super::jobs::now_s() };
    store::whole::write(&p, &serde_json::to_vec(&s)?)?;
    Ok(Some(s))
}

/// The cores kept of a Mac's `cores` while its owner uses it: the share's, else the default; at
/// least one left to the jobs.
pub fn cores(s: Option<&Share>, cores: u32) -> u32 {
    s.and_then(|s| s.cores).unwrap_or(DEFAULT_CORES).min(cores.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_share_is_set_read_defaulted_and_cleared() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(read(d.path()), None);
        assert_eq!(cores(None, 8), 2);
        assert_eq!(cores(None, 1), 0);
        let s = set(d.path(), Some(4), "a test").unwrap();
        assert_eq!(read(d.path()), s);
        assert_eq!(cores(read(d.path()).as_ref(), 8), 4);
        assert_eq!(set(d.path(), None, "a test").unwrap(), None);
        assert_eq!(read(d.path()), None);
        // (A file from before the build's memory: its memory ignored.)
        std::fs::write(d.path().join(FILE), br#"{"cores":3,"mem_mb":4096,"by":"x","at":1}"#).unwrap();
        assert_eq!(cores(read(d.path()).as_ref(), 8), 3);
        std::fs::write(d.path().join(FILE), b"{not json").unwrap();
        assert!(read_checked(d.path()).is_err() && read(d.path()).is_none());
    }
}
