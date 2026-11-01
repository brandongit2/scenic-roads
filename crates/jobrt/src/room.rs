//! A job's disk room (task #421): the owner's room target its agent tells it (`TARGET_ENV`) and the
//! free space past it, which the OSM pass and pass-sets keep as they write. The agent's room policy
//! (its caches freed toward the target, the NAS's reserve) is pipeline's agent::room, which
//! re-exports these.

use std::path::Path;

/// The variable a job is told the owner's room target in (MB; the agent sets it on every job it
/// starts, `Agent::target_env`, 0 for one that keeps none): what the OSM pass and pass-sets keep
/// free past what they write. Unset (a job run by hand): no target.
pub const TARGET_ENV: &str = "SCENIC_ROOM_TARGET_MB";

/// The room target a job's `TARGET_ENV` says (bytes): `v` as MB; unset or unreadable, none (0).
pub fn target_of_env(v: Option<&str>) -> u64 {
    v.and_then(|v| v.trim().parse::<u64>().ok()).map_or(0, |mb| (mb << 20).min(MAX_TARGET))
}

/// This job's room target (bytes; `TARGET_ENV`, 0 unset).
pub fn job_target() -> u64 {
    target_of_env(std::env::var(TARGET_ENV).ok().as_deref())
}

/// The free space on `path`'s disk a job may fill (bytes): what's free less the room target
/// `target` (`job_target`).
pub fn free_past(path: &Path, target: u64) -> u64 {
    free_past_of(free_bytes(path).unwrap_or(0), target)
}

/// The free space `free` has past `target` (none short of it).
pub fn free_past_of(free: u64, target: u64) -> u64 {
    free.saturating_sub(target)
}

/// The largest target (1 PB: a hand-edited file's past any disk), so sums with it can't overflow.
pub const MAX_TARGET: u64 = 1 << 50;

/// Free bytes on the volume holding `path`; with `SCENIC_TEST_DISK_EXTRA_MB` set (the two-Mac
/// test's alone, tools/check/pool-two-macs.sh `extra`: never the real agent's), that many MB more,
/// so a small disk image the test's agent home is on stands for a larger disk,
/// its fills landing in the image, never filling the Mac's own disk.
pub fn free_bytes(path: &Path) -> Option<u64> {
    store::sys::disk_free(path).ok().map(|f| f.saturating_add(test_disk_extra_mb() << 20))
}

/// `SCENIC_TEST_DISK_EXTRA_MB` (0 unset), at most 1 TB: said at the agent's start and in its
/// status, never silent.
pub fn test_disk_extra_mb() -> u64 {
    static EXTRA: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *EXTRA.get_or_init(|| {
        std::env::var("SCENIC_TEST_DISK_EXTRA_MB").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0).min(1 << 20)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A job is told the owner's room target in MB (TARGET_ENV, task #421); unset or unreadable, no
    /// target; the room it reckons is the free space past it.
    #[test]
    fn a_jobs_room_target_is_read_from_its_env() {
        assert_eq!(target_of_env(Some("20480")), 20 << 30);
        assert_eq!((target_of_env(None), target_of_env(Some("x")), target_of_env(Some("0"))), (0, 0, 0));
        // (Over a given free space: the disk's own changes as other work writes, #442.)
        let free = 100 << 30;
        assert_eq!(free_past_of(free, 0), free);
        assert_eq!(free_past_of(free, free + 1), 0);
        assert_eq!(free_past_of(free, 2 << 30), 98 << 30);
        // And read from the disk: none past a target beyond any disk.
        let d = tempfile::tempdir().unwrap();
        assert_eq!(free_past(d.path(), MAX_TARGET), 0);
    }
}
