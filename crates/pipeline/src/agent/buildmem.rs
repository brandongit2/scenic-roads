//! The build's memory (docs/pool.md §7.1): one fixed amount of memory a Mac gives the build's jobs
//! together, set per Mac by `scenic memory <GB>` or the menu bar's Memory submenu, in its agent's
//! folder (`FILE`), which the agent reads each loop (as the disk room target, crate::agent::room).
//! It bounds the Mac's asks (`coord::Ask::mem_mb`: a member's and the lead's own) and the
//! forecast's room for the Mac (crate::agent::forecast's `MacRoom`), within the memory guard's
//! limit (crate::agent::memguard::limit_mb), which stays the hard ceiling (`room_mb`). Not set: the
//! limit alone.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The memory set (MB), who set it ("scenic memory on m1", "the menu bar on m1") and when (unix s).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Memory {
    pub mb: u64,
    pub by: String,
    pub at: u64,
}

/// The setting's file, in the agent's folder.
pub const FILE: &str = "build-memory.json";

/// The memory the build's jobs get when neither a setting nor the Mac's memory is known (MB).
pub const UNKNOWN_MB: u64 = 6144;

/// Sets this Mac's build memory (its agent's folder `home`) to `mb`, or clears it (None: the
/// memory guard's limit alone).
pub fn set(home: &Path, mb: Option<u64>, by: &str) -> Result<Option<Memory>> {
    let p = home.join(FILE);
    let Some(mb) = mb else {
        match std::fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        return Ok(None);
    };
    std::fs::create_dir_all(home)?;
    let m = Memory { mb, by: by.to_string(), at: super::jobs::now_s() };
    crate::whole::write(&p, &serde_json::to_vec(&m)?)?;
    Ok(Some(m))
}

/// This Mac's build memory, when set (a file that doesn't parse, or of 0 MB, is none).
pub fn read(home: &Path) -> Option<Memory> {
    std::fs::read(home.join(FILE)).ok().and_then(|b| serde_json::from_slice::<Memory>(&b).ok()).filter(|m| m.mb > 0)
}

/// The memory the build's jobs get together (MB): the amount set within the memory guard's
/// `limit_mb` (0: unknown), else the limit; neither known, `UNKNOWN_MB`.
pub fn room_mb(set: Option<&Memory>, limit_mb: u64) -> u64 {
    match (set.map(|m| m.mb), limit_mb) {
        (Some(m), 0) => m,
        (Some(m), l) => m.min(l),
        (None, 0) => UNKNOWN_MB,
        (None, l) => l,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_build_memory_is_set_read_bounded_by_the_limit_and_cleared() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(read(d.path()), None);
        // (Not set: the guard's limit, else 6 GB.)
        assert_eq!((room_mb(None, 12_288), room_mb(None, 0)), (12_288, UNKNOWN_MB));
        let m = set(d.path(), Some(10 << 10), "a test").unwrap();
        assert_eq!(read(d.path()), m);
        let m = read(d.path());
        assert_eq!(m.as_ref().map(|m| (m.mb, m.by.as_str())), Some((10_240, "a test")));
        // (The limit stays the ceiling.)
        assert_eq!(
            (room_mb(m.as_ref(), 12_288), room_mb(m.as_ref(), 8192), room_mb(m.as_ref(), 0)),
            (10_240, 8192, 10_240)
        );
        assert_eq!(set(d.path(), None, "a test").unwrap(), None);
        assert_eq!(read(d.path()), None);
        std::fs::write(d.path().join(FILE), b"{not json").unwrap();
        assert_eq!(read(d.path()), None);
        std::fs::write(d.path().join(FILE), br#"{"mb":0,"by":"x","at":1}"#).unwrap();
        assert_eq!(read(d.path()), None);
    }
}
