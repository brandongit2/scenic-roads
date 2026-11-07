//! The free space the steps check before they write (task #916 step 6e, docs/crates.md): a local
//! disk's (`disk_free`; a test's own disk in a crate's unit tests, `out::testing`) and the NAS's
//! past the owner's reserve (`nas_room`, `NAS_RESERVE`), with the file a reserve or a Mac's disk
//! room target is kept in (`Target`), and this Mac's target read (`target`, `RoomTarget`: the map
//! server's mirror reads it, step 7a). The agent's room policy (its caches freed toward the target)
//! is agentcore's agent::room, which re-exports these.

use anyhow::Result;
use jobrt::room::{free_past_of, MAX_TARGET};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The owner's disk room target on this Mac (`scenic room <GB>`, the build page's chart): the
/// free space its agent keeps (pipeline's agent::room, The room target). In the agent's folder
/// (`TARGET`): each Mac's disk is its own. The NAS reserve is kept in the same form.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// The free space to keep (bytes).
    pub bytes: u64,
    /// Who set it, in words ("scenic room on m4", "the menu bar on m4"), and when (unix seconds).
    pub by: String,
    pub at: u64,
}

/// A target's file's bytes (none: unreadable, or 0 bytes; at most `MAX_TARGET`).
pub fn parse_target(b: &[u8]) -> Option<Target> {
    serde_json::from_slice::<Target>(b)
        .ok()
        .filter(|t| t.bytes > 0)
        .map(|t| Target { bytes: t.bytes.min(MAX_TARGET), ..t })
}

/// The target's file, in the agent's folder.
pub const TARGET: &str = "room-target.json";

/// This Mac's disk room target, when one is set (one that doesn't parse, or of 0 bytes, is none;
/// one past `MAX_TARGET` is that).
pub fn target(home: &Path) -> Option<Target> {
    read_target(&home.join(TARGET))
}

/// A target's file at `p` (none: missing, unreadable, or 0 bytes; at most `MAX_TARGET`).
fn read_target(p: &Path) -> Option<Target> {
    parse_target(&std::fs::read(p).ok()?)
}

/// This Mac's disk room target as another process reads it (bytes, 0 when none): the target's file
/// in the agent's folder, read again at most every `RoomTarget::EVERY`, so asking is cheap. The map
/// server's mirror keeps at least this free (docs/macs.md, Mirror).
pub struct RoomTarget {
    home: std::path::PathBuf,
    last: std::sync::Mutex<Option<(std::time::Instant, u64)>>,
}

impl RoomTarget {
    /// How long a read holds: a target set with `scenic room` counts within this.
    pub const EVERY: std::time::Duration = std::time::Duration::from_secs(5);

    /// The target of the agent whose folder is `home`.
    pub fn new(home: &Path) -> RoomTarget {
        RoomTarget { home: home.to_owned(), last: std::sync::Mutex::new(None) }
    }

    pub fn bytes(&self) -> u64 {
        let mut g = self.last.lock().unwrap_or_else(|e| e.into_inner());
        match *g {
            Some((at, b)) if at.elapsed() < Self::EVERY => b,
            _ => {
                let b = target(&self.home).map_or(0, |t| t.bytes);
                *g = Some((std::time::Instant::now(), b));
                b
            }
        }
    }
}

/// The owner's NAS reserve, pool-wide (`scenic nas-room`): the free space the NAS keeps past what the
/// steps that check before writing there would write (bld-fetch). One file on the NAS, written by
/// whichever Mac sets it, read by every job; none (the default): the NAS's free space alone.
pub const NAS_RESERVE: &str = "state/pool/nas-room.json";

/// Sets the NAS reserve under the project folder `root` to `bytes`, or clears it (None).
pub fn set_nas_reserve(root: &store::nasfs::NasRoot, bytes: Option<u64>, by: &str) -> Result<Option<Target>> {
    let p = root.join(NAS_RESERVE);
    let Some(bytes) = bytes else {
        match p.remove_file() {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        return Ok(None);
    };
    if let Some(d) = p.parent() {
        d.create_dir_all()?;
    }
    let t = Target { bytes, by: by.to_string(), at: jobrt::jobs::now_s() };
    p.write_whole(serde_json::to_vec(&t)?)?;
    Ok(Some(t))
}

/// The NAS reserve as set under `root`, when one is.
pub fn nas_reserve(root: &store::nasfs::NasRoot) -> Option<Target> {
    parse_target(&root.join(NAS_RESERVE).read().ok()?)
}

/// The NAS's free space a step may fill (bytes): what's free under `root` less the reserve.
pub fn nas_room(root: &store::nasfs::NasRoot) -> std::io::Result<u64> {
    Ok(free_past_of(nas_free(root)?, nas_reserve(root).map_or(0, |t| t.bytes)))
}

/// The NAS's free space: its mount's statfs (tests/nas_access.rs).
pub fn nas_free(root: &store::nasfs::NasRoot) -> std::io::Result<u64> {
    disk_free(&root.unchecked_path())
}

/// The free space on the disk holding `path` (a local disk).
pub fn disk_free(path: &Path) -> std::io::Result<u64> {
    // (A test's disk is its own, never this Mac's: what's free here, with other suites running,
    // decides nothing. 1 TB unless the test says. A crate's unit tests, `out::testing`: as
    // `cfg(test)` decided it while this was pipeline's.)
    if crate::out::testing() {
        return Ok(TEST_FREE.with(|c| c.get()).unwrap_or(1 << 40));
    }
    jobrt::room::free_bytes(path).ok_or_else(std::io::Error::last_os_error)
}

use test_disk::TEST_FREE;

mod test_disk {
    thread_local! {
        /// A test's disk, so what it asserts doesn't hang on this Mac's: its free bytes (read only
        /// in a crate's unit tests, `disk_free`).
        pub static TEST_FREE: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
    }
}

/// A test's disk (`TEST_FREE`), for other crates' unit tests (pipeline's agent).
#[cfg(any(test, feature = "testkit"))]
pub mod testkit {
    pub use super::test_disk::TEST_FREE;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_room_target_is_read_once_then_held() {
        let home = tempfile::tempdir().unwrap();
        let t = RoomTarget::new(home.path());
        assert_eq!(t.bytes(), 0, "none set");
        std::fs::write(home.path().join(TARGET), br#"{"bytes":1073741824,"by":"t","at":1}"#).unwrap();
        assert_eq!(t.bytes(), 0, "held");
        assert_eq!(RoomTarget::new(home.path()).bytes(), 1 << 30);
        *t.last.lock().unwrap() = None;
        assert_eq!(t.bytes(), 1 << 30, "read again");
    }

    /// The NAS reserve: kept on the NAS, read back, its room the free space past it; off, none.
    #[test]
    fn the_nas_reserve_is_kept_on_the_nas() {
        let d = tempfile::tempdir().unwrap();
        TEST_FREE.with(|c| c.set(Some(100 << 30)));
        assert_eq!(nas_reserve(&store::nasfs::NasRoot::local_for_test(d.path())), None);
        assert_eq!(nas_room(&store::nasfs::NasRoot::local_for_test(d.path())).unwrap(), 100 << 30);
        let t = set_nas_reserve(&store::nasfs::NasRoot::local_for_test(d.path()), Some(30 << 30), "a test")
            .unwrap()
            .unwrap();
        assert!(d.path().join(NAS_RESERVE).is_file());
        assert_eq!(nas_reserve(&store::nasfs::NasRoot::local_for_test(d.path())), Some(t));
        assert_eq!(nas_room(&store::nasfs::NasRoot::local_for_test(d.path())).unwrap(), 70 << 30);
        set_nas_reserve(&store::nasfs::NasRoot::local_for_test(d.path()), Some(200 << 30), "a test").unwrap();
        assert_eq!(nas_room(&store::nasfs::NasRoot::local_for_test(d.path())).unwrap(), 0);
        assert_eq!(set_nas_reserve(&store::nasfs::NasRoot::local_for_test(d.path()), None, "a test").unwrap(), None);
        assert_eq!(
            (
                nas_reserve(&store::nasfs::NasRoot::local_for_test(d.path())),
                nas_room(&store::nasfs::NasRoot::local_for_test(d.path())).unwrap()
            ),
            (None, 100 << 30)
        );
        TEST_FREE.with(|c| c.set(None));
    }
}
