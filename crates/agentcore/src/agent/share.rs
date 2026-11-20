//! The in-use share (docs/pool.md §7.1): the cores a Mac's agent keeps free of its jobs for its
//! owner while they use the Mac (used within five minutes). Set by `scenic in-use` in the agent's
//! folder, which the agent reads each loop (as the disk room target, crate::agent::room); not set,
//! the default. (The memory is kept by the memory floor, crate::agent::memfloor, whether the owner
//! uses the Mac or not.)
//!
//! And the **test hold** (task #1570): the cores publish's tests on the test Mac hold while they
//! run (`HOLD_FILE`, written by tools/app/remote-tests.sh as they start and removed as its run
//! ends), kept free of the jobs as the share is while the owner uses the Mac (`kept`). A hold
//! whose process is gone (killed outright, no trap run) holds nothing.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The share as set: its cores where said, who set it and when (unix seconds). (A file from before
/// task #390 may say `mem_mb` too: ignored.)
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

/// Publish's tests' hold of this Mac: the cores they run on (their test threads), the process
/// of their run (remote-tests.sh's on the test Mac) and when it was written (unix seconds).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hold {
    pub cores: u32,
    pub pid: i32,
    pub at: u64,
}

/// The test hold's file, in the agent's folder.
pub const HOLD_FILE: &str = "test-hold.json";

/// The test hold in `home` while it holds: its file read, and its process alive, started by the
/// time it was written (a PID used again since is another process's). None otherwise: no file, one
/// that can't be read, its process gone.
pub fn hold(home: &Path) -> Option<Hold> {
    let h: Hold = serde_json::from_slice(&std::fs::read(home.join(HOLD_FILE)).ok()?).ok()?;
    jobrt::sys::process_start(h.pid).is_some_and(|s| s <= h.at).then_some(h)
}

/// The cores this Mac keeps free of its jobs now, of its `cores`: the share's while its owner uses
/// it (`active`), the test hold's while it holds, the more of the two; one left to the jobs at least.
pub fn kept(s: Option<&Share>, hold: Option<&Hold>, active: bool, cores: u32) -> u32 {
    let owner = if active { self::cores(s, cores) } else { 0 };
    owner.max(hold.map_or(0, |h| h.cores)).min(cores.saturating_sub(1))
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

    /// Task #1570: the test hold holds while its process runs, and is kept as the share is: the
    /// more of the two (the share's only while the owner uses the Mac), one core left to the jobs.
    /// A hold whose process is gone (killed outright), or whose PID is now a process started after
    /// it was written, holds nothing; nor an unreadable file.
    #[test]
    fn the_test_hold_holds_while_its_process_runs() {
        let d = tempfile::tempdir().unwrap();
        let put = |h: &Hold| std::fs::write(d.path().join(HOLD_FILE), serde_json::to_vec(h).unwrap()).unwrap();
        assert_eq!(hold(d.path()), None);
        let me = Hold { cores: 9, pid: std::process::id() as i32, at: super::super::jobs::now_s() };
        put(&me);
        assert_eq!(hold(d.path()), Some(me.clone()));
        let s = Share { cores: Some(4), by: "t".into(), at: 1 };
        assert_eq!(kept(Some(&s), Some(&me), false, 14), 9, "the owner away: the hold's");
        assert_eq!(kept(Some(&s), None, true, 14), 4, "no hold: the share's");
        assert_eq!(kept(Some(&s), Some(&Hold { cores: 3, ..me.clone() }), true, 14), 4, "the more");
        assert_eq!(kept(None, Some(&Hold { cores: 14, ..me.clone() }), false, 14), 13, "a core left to the jobs");
        assert_eq!(kept(Some(&s), None, false, 14), 0);
        // Its process gone: nothing held.
        let mut c = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let gone = c.id() as i32;
        c.wait().unwrap();
        put(&Hold { pid: gone, ..me.clone() });
        assert_eq!(hold(d.path()), None, "its process gone");
        // A live PID whose process started after the hold was written: another's.
        put(&Hold { at: 1, ..me.clone() });
        assert_eq!(hold(d.path()), None, "its PID another process's");
        std::fs::write(d.path().join(HOLD_FILE), b"{not json").unwrap();
        assert_eq!(hold(d.path()), None);
    }
}
