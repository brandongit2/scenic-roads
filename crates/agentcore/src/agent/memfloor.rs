//! The memory floor (docs/pool.md §7.1): the memory a Mac keeps available, whatever runs on it
//! (the build's jobs, the agent, the owner's apps): "keep at least N GB available on this Mac",
//! as the disk room target keeps a minimum of free disk. Set per Mac by `scenic memory <GB>` or the
//! build page, in its agent's folder (`FILE`), which the agent reads each loop; not set, the Mac's
//! default (`default_mb`), shown as the setting's value. No cap or reserve stands behind it: the
//! memory guard (crate::agent::memguard) starts and gives back jobs by it alone, as set.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The floor set (MB), who set it ("scenic memory on m1", "the build page (…)") and when (unix s).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Floor {
    pub mb: u64,
    pub by: String,
    pub at: u64,
}

/// The setting's file, in the agent's folder.
pub const FILE: &str = "memory-floor.json";

/// The file of the setting it replaced (task #932): the build's memory, a cap on its jobs together
/// (`{"mb", "by", "at"}`), which `migrate` removes.
pub const OLD_FILE: &str = "build-memory.json";

/// The floor of a Mac of `total_mb` when none is set (MB): an eighth of its memory (6 GB of the
/// M4's 48, 4 of the Mac mini's 32, 2 of the M1's 16). On 4 Nov each of the three, busy and
/// compressing, held a file cache of 11–12 % of its memory that macOS kept rather than dropped
/// (the M4 5.4 GB of 48, the Mac mini 3.6 of 32, the M1 1.8 of 16: docs/pool.md §7.1), so its
/// available memory (crate::sys::memory_available) fell below an eighth once macOS compressed
/// rather than dropped cache: the floor sits just above where the Mac starts to compress. 0 for a
/// Mac whose memory isn't known.
pub fn default_mb(total_mb: u64) -> u64 {
    total_mb / 8
}

/// The floor in force (MB): the one set, else the default of a Mac of `total_mb`.
pub fn floor_mb(set: Option<&Floor>, total_mb: u64) -> u64 {
    set.map_or_else(|| default_mb(total_mb), |f| f.mb)
}

/// Sets this Mac's floor (its agent's folder `home`) to `mb`, or clears it (None: the default).
pub fn set(home: &Path, mb: Option<u64>, by: &str) -> Result<Option<Floor>> {
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
    let f = Floor { mb, by: by.to_string(), at: super::jobs::now_s() };
    store::whole::write(&p, &serde_json::to_vec(&f)?)?;
    Ok(Some(f))
}

/// This Mac's floor, when set (a file that doesn't parse is none).
pub fn read(home: &Path) -> Option<Floor> {
    std::fs::read(home.join(FILE)).ok().and_then(|b| serde_json::from_slice::<Floor>(&b).ok())
}

/// The old setting's file in `home` (`OLD_FILE`), removed: a cap on the jobs has no floor it
/// stands for (what the owner's apps hold isn't available memory), so the Mac keeps its default
/// floor; a floor set already stays. What it said, in words, for the agent's log and the guard's
/// last word (None: there was none).
pub fn migrate(home: &Path, total_mb: u64) -> Option<String> {
    let p = home.join(OLD_FILE);
    let b = std::fs::read(&p).ok()?;
    std::fs::remove_file(&p).ok()?;
    #[derive(Deserialize)]
    struct Old {
        mb: u64,
        #[serde(default)]
        by: String,
    }
    let gb = |mb: u64| format!("{:.1} GB", mb as f64 / 1024.0);
    let was = match serde_json::from_slice::<Old>(&b) {
        Ok(o) if !o.by.is_empty() => format!("the build's jobs held to {} together (set by {})", gb(o.mb), o.by),
        Ok(o) => format!("the build's jobs held to {} together", gb(o.mb)),
        Err(_) => "a build memory that didn't read".to_string(),
    };
    let now = match read(home) {
        Some(f) => format!("its memory floor of {} stays", gb(f.mb)),
        None => format!(
            "this Mac keeps its default floor of {} available (`scenic memory <GB>` sets another)",
            gb(default_mb(total_mb))
        ),
    };
    Some(format!("the build memory setting ({was}) is replaced by the memory floor: {now}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_floor_is_set_read_defaulted_and_cleared() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(read(d.path()), None);
        // (Not set: an eighth of the Mac; its memory unknown, none.)
        assert_eq!((floor_mb(None, 48 << 10), floor_mb(None, 16 << 10), floor_mb(None, 0)), (6 << 10, 2 << 10, 0));
        let f = set(d.path(), Some(10 << 10), "a test").unwrap();
        assert_eq!(read(d.path()), f);
        assert_eq!(floor_mb(read(d.path()).as_ref(), 48 << 10), 10 << 10, "as set, whatever the default");
        assert_eq!(set(d.path(), None, "a test").unwrap(), None);
        assert_eq!(read(d.path()), None);
        std::fs::write(d.path().join(FILE), b"{not json").unwrap();
        assert_eq!(read(d.path()), None);
    }

    /// An old build memory (a cap on the jobs) is removed, said, and the default floor stands; a
    /// floor set already stays; with none, nothing.
    #[test]
    fn an_old_build_memory_is_migrated_to_the_default_floor() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(migrate(d.path(), 48 << 10), None);
        std::fs::write(d.path().join(OLD_FILE), br#"{"mb":24576,"by":"the build page (100.70.85.80)","at":1}"#)
            .unwrap();
        let said = migrate(d.path(), 48 << 10).unwrap();
        assert_eq!(said, "the build memory setting (the build's jobs held to 24.0 GB together (set by the build page (100.70.85.80))) is replaced by the memory floor: this Mac keeps its default floor of 6.0 GB available (`scenic memory <GB>` sets another)");
        assert!(!d.path().join(OLD_FILE).exists() && read(d.path()).is_none());
        assert_eq!(migrate(d.path(), 48 << 10), None, "once");
        set(d.path(), Some(8 << 10), "t").unwrap();
        std::fs::write(d.path().join(OLD_FILE), br#"{"mb":10240,"by":"","at":1}"#).unwrap();
        assert!(migrate(d.path(), 48 << 10).unwrap().ends_with("its memory floor of 8.0 GB stays"));
        assert_eq!(read(d.path()).map(|f| f.mb), Some(8 << 10));
    }
}
