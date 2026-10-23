//! The owner's settings (docs/workers.md §7): this Mac's in-use share (crate::agent::share), build
//! memory (crate::agent::buildmem) and disk room target (crate::agent::room::Target), and the pool's
//! NAS reserve (crate::agent::room::NAS_RESERVE). Each is set through one function here, whoever
//! asks: `scenic in-use`, `scenic memory`, `scenic room`, `scenic nas-room`, and the build page's
//! POST /work/settings (crate::coord), which reads them back with `view`.

use super::{buildmem, room, share};
use anyhow::{Context, Result};
use std::path::Path;

/// This Mac's cores.
pub fn cores() -> u32 {
    std::thread::available_parallelism().map_or(4, |n| n.get() as u32)
}

/// A size asked in GB ("12", "12GB", "12 gb"), 1 GB or more; `what` names it in the error.
fn gb(s: &str, what: &str) -> Result<f64> {
    s.trim()
        .trim_end_matches("GB")
        .trim_end_matches("gb")
        .trim()
        .parse()
        .ok()
        .filter(|g: &f64| g.is_finite() && *g >= 1.0)
        .with_context(|| format!("{what}: <GB> or off, not {s}: 1 GB or more"))
}

/// Sets the in-use share in the agent's folder `home`: `cores` kept free of `cores()` (0 to one
/// less), or None: the default again.
pub fn set_in_use(home: &Path, cores_asked: Option<u32>, by: &str) -> Result<Option<share::Share>> {
    let Some(n) = cores_asked else {
        return share::set(home, None, "");
    };
    let cores = cores();
    anyhow::ensure!(n < cores, "the in-use share: 0 to {} cores, not {n}", cores - 1);
    anyhow::ensure!(home.is_dir(), "no agent here ({} isn't there)", home.display());
    share::set(home, Some(n), by)
}

/// Sets the build's memory in `home`: `what` in GB (at most this Mac's memory), or "off": the
/// memory guard's limit alone.
pub fn set_memory(home: &Path, what: &str, by: &str) -> Result<Option<buildmem::Memory>> {
    if what == "off" {
        return buildmem::set(home, None, by);
    }
    let gb = gb(what, "the build memory")?;
    let mb = (gb * 1024.0) as u64;
    let total_mb = crate::sys::memsize().map_or(0, |b| b >> 20);
    anyhow::ensure!(
        total_mb == 0 || mb <= total_mb,
        "{gb} GB is more than this Mac has ({:.0} GB)",
        total_mb as f64 / 1024.0
    );
    anyhow::ensure!(home.is_dir(), "no agent here ({} isn't there)", home.display());
    buildmem::set(home, Some(mb), by)
}

/// Sets this Mac's disk room target in `home`: `what` in GB (less than its disk holds), or "off".
pub fn set_room(home: &Path, what: &str, by: &str) -> Result<Option<room::Target>> {
    if what == "off" {
        return room::set_target(home, None, by);
    }
    let bytes = (gb(what, "the disk room target")? * (1u64 << 30) as f64) as u64;
    anyhow::ensure!(home.is_dir(), "no agent here ({} isn't there)", home.display());
    if let Some(n) = disk_size(home) {
        anyhow::ensure!(
            bytes < n,
            "a target of {} is more than this disk holds ({})",
            room::size(bytes),
            room::size(n)
        );
    }
    room::set_target(home, Some(bytes), by)
}

/// Sets the pool's NAS reserve under the project folder `root`: `what` in GB, or "off".
pub fn set_nas_room(root: &Path, what: &str, by: &str) -> Result<Option<room::Target>> {
    if what == "off" {
        return room::set_nas_reserve(root, None, by);
    }
    let bytes = (gb(what, "the NAS reserve")? * (1u64 << 30) as f64) as u64;
    room::set_nas_reserve(root, Some(bytes), by)
}

/// The size of the disk holding `p` (bytes).
pub fn disk_size(p: &Path) -> Option<u64> {
    let c = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(c.as_ptr(), &mut s) } == 0).then(|| s.f_blocks as u64 * s.f_frsize as u64)
}

/// The settings as they stand, for the build page: this Mac's (in its agent's folder `home`) and
/// the NAS reserve (under `root`; null while the NAS is away). Sizes in bytes, memory in MB.
pub fn view(home: &Path, root: Option<&Path>) -> serde_json::Value {
    let total_mb = crate::sys::memsize().map_or(0, |b| b >> 20);
    let limit = super::memguard::limit_mb(total_mb);
    let mem = buildmem::read(home);
    let sh = share::read(home);
    let cores = cores();
    serde_json::json!({
        "in_use": {
            "cores": share::cores(sh.as_ref(), cores),
            "of": cores,
            "set": sh.is_some(),
            "default": share::DEFAULT_CORES,
            "by": sh.as_ref().map(|s| s.by.clone()),
            "at": sh.as_ref().map(|s| s.at),
        },
        "memory": {
            "mb": buildmem::room_mb(mem.as_ref(), limit),
            "limit_mb": limit,
            "total_mb": total_mb,
            "set": mem.is_some(),
            "by": mem.as_ref().map(|m| m.by.clone()),
            "at": mem.as_ref().map(|m| m.at),
        },
        "room": target_view(room::target(home), room::disk_free(home).ok(), disk_size(home)),
        "nas_room": root.map(|r| target_view(room::nas_reserve(r), room::disk_free(r).ok(), None)),
    })
}

fn target_view(t: Option<room::Target>, free: Option<u64>, disk: Option<u64>) -> serde_json::Value {
    serde_json::json!({
        "bytes": t.as_ref().map(|t| t.bytes),
        "by": t.as_ref().map(|t| t.by.clone()),
        "at": t.as_ref().map(|t| t.at),
        "free": free,
        "disk": disk,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_setting_checks_what_it_is_asked_and_off_clears_it() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        assert!(set_in_use(h, Some(cores()), "t").is_err(), "a core must be left to the jobs");
        assert_eq!(set_in_use(h, Some(0), "t").unwrap().unwrap().cores, Some(0));
        assert!(set_in_use(h, None, "t").unwrap().is_none() && share::read(h).is_none());
        for bad in ["0.5", "x", "NaN", "inf", "-3"] {
            assert!(set_memory(h, bad, "t").is_err(), "{bad}");
            assert!(set_room(h, bad, "t").is_err(), "{bad}");
            assert!(set_nas_room(h, bad, "t").is_err(), "{bad}");
        }
        assert!(set_memory(h, "1000000", "t").is_err(), "more than this Mac has");
        assert!(set_room(h, "1000000000", "t").is_err(), "more than the disk holds");
        assert_eq!(set_memory(h, "2 GB", "t").unwrap().unwrap().mb, 2048);
        assert_eq!(set_room(h, "1gb", "t").unwrap().unwrap().bytes, 1 << 30);
        assert_eq!(set_nas_room(h, "3", "t").unwrap().unwrap().bytes, 3 << 30);
        let v = view(h, Some(h));
        assert_eq!(v["memory"]["set"], true);
        assert_eq!(v["room"]["bytes"], 1u64 << 30);
        assert_eq!(v["nas_room"]["bytes"], 3u64 << 30);
        assert!(set_memory(h, "off", "t").unwrap().is_none() && buildmem::read(h).is_none());
        assert!(set_room(h, "off", "t").unwrap().is_none() && room::target(h).is_none());
        assert!(set_nas_room(h, "off", "t").unwrap().is_none() && room::nas_reserve(h).is_none());
        assert!(view(h, None)["nas_room"].is_null());
        let gone = h.join("nope");
        assert!(set_room(&gone, "2", "t").is_err() && set_memory(&gone, "2", "t").is_err());
    }
}
