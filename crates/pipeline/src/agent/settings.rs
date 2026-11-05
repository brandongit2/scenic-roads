//! The owner's settings (docs/workers.md §7): this Mac's in-use share (crate::agent::share), memory
//! floor (crate::agent::memfloor) and disk room target (crate::agent::room::Target), the pool's
//! NAS reserve (crate::agent::room::NAS_RESERVE), batch length and overdue factor (the lead's
//! settings `batch-minutes` and `overdue-factor`, crate::coord::asks), and the dev sweep's margin
//! (`SWEEP_MARGIN`). Each is set through one function here, whoever asks: `scenic in-use`, `scenic
//! memory`, `scenic room`, `scenic nas-room`, `scenic pool set batch-minutes|overdue-factor`, and the
//! build page's
//! POST /work/settings (crate::coord), which reads them back with `view` (the sweep's margin: the
//! build page alone). The map server's mirror
//! reads the room target through `RoomTarget` (its reserve is the larger of the two).

use super::{memfloor, room, share};
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

/// Sets this Mac's memory floor in `home`, the memory it keeps available: `what` in GB (less than
/// this Mac's memory), or "off": its default again.
pub fn set_memory(home: &Path, what: &str, by: &str) -> Result<Option<memfloor::Floor>> {
    if what == "off" {
        return memfloor::set(home, None, by);
    }
    let gb = gb(what, "the memory floor")?;
    let mb = (gb * 1024.0) as u64;
    let total_mb = crate::sys::memsize().map_or(0, |b| b >> 20);
    anyhow::ensure!(
        total_mb == 0 || mb < total_mb,
        "a floor of {gb} GB is all this Mac has or more ({:.0} GB): no job could start",
        total_mb as f64 / 1024.0
    );
    anyhow::ensure!(home.is_dir(), "no agent here ({} isn't there)", home.display());
    memfloor::set(home, Some(mb), by)
}

/// Sets this Mac's disk room target in `home`: `what` in GB (less than its disk holds), or "off".
pub fn set_room(home: &Path, what: &str, by: &str) -> Result<Option<room::Target>> {
    if what == "off" {
        return room::set_target(home, None, by);
    }
    let bytes = (gb(what, "the disk room target")? * (1u64 << 30) as f64) as u64;
    anyhow::ensure!(home.is_dir(), "no agent here ({} isn't there)", home.display());
    if let Ok(n) = store::sys::disk_size(home) {
        anyhow::ensure!(
            bytes < n,
            "a target of {} is more than this disk holds ({})",
            room::size(bytes),
            room::size(n)
        );
    }
    room::set_target(home, Some(bytes), by)
}

/// The dev sweep's margin (tools/dev/sweep-targets.sh, tools/dev/testing.md): the sweep frees the
/// agents' build caches on this Mac once its free disk falls below the room target plus this, so
/// dev builds never hold the pool's jobs there. In the agent's folder, as the room target
/// ({bytes, by, at}, room::Target); none: `SWEEP_MARGIN_DEFAULT_GB`.
pub const SWEEP_MARGIN: &str = "sweep-margin.json";

/// The sweep's margin when none is set (GB). tools/dev/sweep-targets.sh reads it from this line.
pub const SWEEP_MARGIN_DEFAULT_GB: u64 = 10;

/// The sweep's margin as set in `home` (none: not set, or unreadable).
pub fn sweep_margin(home: &Path) -> Option<room::Target> {
    serde_json::from_slice(&std::fs::read(home.join(SWEEP_MARGIN)).ok()?).ok()
}

/// Sets the sweep's margin in `home`: `what` in GB (less than its disk holds), or "off": the default again.
pub fn set_sweep_margin(home: &Path, what: &str, by: &str) -> Result<Option<room::Target>> {
    let p = home.join(SWEEP_MARGIN);
    if what == "off" {
        match std::fs::remove_file(&p) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => return Ok(None),
        }
    }
    let bytes = (gb(what, "the sweep's margin")? * (1u64 << 30) as f64) as u64;
    anyhow::ensure!(home.is_dir(), "no agent here ({} isn't there)", home.display());
    if let Ok(n) = store::sys::disk_size(home) {
        anyhow::ensure!(
            bytes < n,
            "a margin of {} is more than this disk holds ({})",
            room::size(bytes),
            room::size(n)
        );
    }
    let t = room::Target { bytes, by: by.to_string(), at: super::jobs::now_s() };
    store::whole::write(&p, &serde_json::to_vec(&t)?)?;
    Ok(Some(t))
}

/// Sets the pool's NAS reserve under the project folder `root`: `what` in GB, or "off".
pub fn set_nas_room(root: &store::nasfs::NasRoot, what: &str, by: &str) -> Result<Option<room::Target>> {
    if what == "off" {
        return room::set_nas_reserve(root, None, by);
    }
    let bytes = (gb(what, "the NAS reserve")? * (1u64 << 30) as f64) as u64;
    room::set_nas_reserve(root, Some(bytes), by)
}

/// Asks the lead to set the pool's batch length (task #927, docs/pool.md §7.7, Batch length): `what`
/// in minutes (a minute to a day: jobrt::settings::BATCH_RANGE), or "off": its default again
/// (crate::coord::JOB_S). Asked of this Mac's agent (its folder `home`), which sends it to the lead
/// with its next beat (crate::control::request_setting), as every pool setting is.
pub fn set_batch(home: &Path, what: &str, by: &str) -> Result<()> {
    use crate::coord::asks::{SettingAsk, BATCH};
    let value = match what.trim() {
        "off" => serde_json::Value::Null,
        m => m
            .parse::<u64>()
            .ok()
            .filter(|m| jobrt::settings::BATCH_RANGE.contains(m))
            .with_context(|| format!("batch-minutes: 1 to 1440 minutes or off, not {what}"))?
            .into(),
    };
    anyhow::ensure!(home.is_dir(), "no agent here ({} isn't there)", home.display());
    let a = SettingAsk { name: BATCH.into(), value, by: by.to_string(), at: super::jobs::now_s() };
    crate::control::request_setting(home, &a)
}

/// The pool's batch length as this Mac's agent last had the pool's settings from the lead (in
/// `home`; `had` false: not yet), in minutes.
fn batch_view(home: &Path) -> serde_json::Value {
    let s = crate::control::read_pool_settings(home);
    let set = s.as_ref().and_then(|s| s.values.get(crate::coord::asks::BATCH)).filter(|v| !v.value.is_null());
    let default = crate::coord::JOB_S / 60;
    serde_json::json!({
        "minutes": s.as_ref().and_then(|s| s.batch_s()).map_or(default, |b| b / 60),
        "default": default,
        "set": set.is_some(),
        "by": set.map(|v| v.by.clone()),
        "at": set.map(|v| v.at),
        "had": s.is_some(),
    })
}

/// The overdue factor asked ("3", "3x", "3×"): a number within `asks::OVERDUE_RANGE`, or "off"
/// (null: the default again), as the pool's setting takes it.
fn overdue_value(what: &str) -> Result<serde_json::Value> {
    use crate::coord::asks::OVERDUE_RANGE;
    if what.trim() == "off" {
        return Ok(serde_json::Value::Null);
    }
    let n = what
        .trim()
        .trim_end_matches(['x', '×'])
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|n| OVERDUE_RANGE.contains(n))
        .with_context(|| {
        format!(
            "the overdue factor: a number from {} to {}, or off, not {what}",
            OVERDUE_RANGE.start(),
            OVERDUE_RANGE.end()
        )
    })?;
    Ok(n.into())
}

/// Asks the pool's overdue factor (agent::flags) of this Mac's agent in `home`, which sends it to
/// the lead with its next beat (as `scenic pool set overdue-factor`): `what` as `overdue_value`
/// takes it.
pub fn set_overdue(home: &Path, what: &str, by: &str) -> Result<()> {
    let value = overdue_value(what)?;
    anyhow::ensure!(home.is_dir(), "no agent here ({} isn't there)", home.display());
    let ask = crate::coord::asks::SettingAsk {
        name: crate::coord::asks::OVERDUE.into(),
        value,
        by: by.to_string(),
        at: super::jobs::now_s(),
    };
    crate::control::request_setting(home, &ask)
}

/// The pool's overdue factor as this Mac's agent last had the pool's settings from the lead (in
/// `home`; `had` false: not yet).
fn overdue_view(home: &Path) -> serde_json::Value {
    let s = crate::control::read_pool_settings(home);
    let (factor, set) = s.as_ref().map_or((crate::coord::asks::OVERDUE_DEFAULT, false), |s| s.overdue());
    let v = s.as_ref().and_then(|s| s.values.get(crate::coord::asks::OVERDUE)).filter(|_| set);
    serde_json::json!({
        "factor": factor,
        "default": crate::coord::asks::OVERDUE_DEFAULT,
        "set": set,
        "by": v.map(|v| v.by.clone()),
        "at": v.map(|v| v.at),
        "had": s.is_some(),
    })
}

/// The settings as they stand, for the build page: this Mac's (in its agent's folder `home`), the
/// pool's NAS reserve (under `root`; null while the NAS is away), and its batch length and overdue
/// factor (as this Mac's agent last had them from the lead). Sizes in bytes, memory in MB.
pub fn view(home: &Path, root: Option<&store::nasfs::NasRoot>) -> serde_json::Value {
    let total_mb = crate::sys::memsize().map_or(0, |b| b >> 20);
    let mem = memfloor::read(home);
    let sh = share::read(home);
    let cores = cores();
    let margin = sweep_margin(home);
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
            "mb": memfloor::floor_mb(mem.as_ref(), total_mb),
            "default_mb": memfloor::default_mb(total_mb),
            "total_mb": total_mb,
            "set": mem.is_some(),
            "by": mem.as_ref().map(|m| m.by.clone()),
            "at": mem.as_ref().map(|m| m.at),
        },
        "room": target_view(room::target(home), room::disk_free(home).ok(), store::sys::disk_size(home).ok()),
        "sweep_margin": {
            "bytes": margin.as_ref().map_or(SWEEP_MARGIN_DEFAULT_GB << 30, |m| m.bytes),
            "default": SWEEP_MARGIN_DEFAULT_GB << 30,
            "set": margin.is_some(),
            "by": margin.as_ref().map(|m| m.by.clone()),
            "at": margin.as_ref().map(|m| m.at),
        },
        "nas_room": root.map(|r| target_view(room::nas_reserve(r), room::nas_free(r).ok(), None)),
        "batch": batch_view(home),
        "overdue": overdue_view(home),
    })
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
                let b = room::target(&self.home).map_or(0, |t| t.bytes);
                *g = Some((std::time::Instant::now(), b));
                b
            }
        }
    }
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
    fn the_room_target_is_read_once_then_held() {
        let home = tempfile::tempdir().unwrap();
        let t = RoomTarget::new(home.path());
        assert_eq!(t.bytes(), 0, "none set");
        set_room(home.path(), "1", "t").unwrap();
        assert_eq!(t.bytes(), 0, "held");
        assert_eq!(RoomTarget::new(home.path()).bytes(), 1 << 30);
        *t.last.lock().unwrap() = None;
        assert_eq!(t.bytes(), 1 << 30, "read again");
    }

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
            assert!(set_sweep_margin(h, bad, "t").is_err(), "{bad}");
            assert!(set_nas_room(&store::nasfs::NasRoot::local_for_test(h), bad, "t").is_err(), "{bad}");
        }
        assert!(set_memory(h, "1000000", "t").is_err(), "more than this Mac has");
        let all = (crate::sys::memsize().unwrap() >> 30).to_string();
        assert!(set_memory(h, &all, "t").is_err(), "all this Mac has: no job could start");
        assert!(set_room(h, "1000000000", "t").is_err(), "more than the disk holds");
        assert!(set_sweep_margin(h, "1000000000", "t").is_err(), "more than the disk holds");
        assert_eq!(view(h, None)["sweep_margin"]["bytes"], SWEEP_MARGIN_DEFAULT_GB << 30, "none set: the default");
        assert_eq!(set_sweep_margin(h, "4", "t").unwrap().unwrap().bytes, 4 << 30);
        assert_eq!(set_memory(h, "2 GB", "t").unwrap().unwrap().mb, 2048);
        assert_eq!(set_room(h, "1gb", "t").unwrap().unwrap().bytes, 1 << 30);
        assert_eq!(set_nas_room(&store::nasfs::NasRoot::local_for_test(h), "3", "t").unwrap().unwrap().bytes, 3 << 30);
        let v = view(h, Some(&store::nasfs::NasRoot::local_for_test(h)));
        assert_eq!(v["memory"]["set"], true);
        assert_eq!(v["room"]["bytes"], 1u64 << 30);
        assert_eq!(v["nas_room"]["bytes"], 3u64 << 30);
        assert!(set_memory(h, "off", "t").unwrap().is_none() && memfloor::read(h).is_none());
        let v = view(h, None);
        assert!(v["memory"]["set"] == false && v["memory"]["mb"] == v["memory"]["default_mb"], "{v}");
        assert!(set_room(h, "off", "t").unwrap().is_none() && room::target(h).is_none());
        assert!(view(h, None)["sweep_margin"]["bytes"] == 4u64 << 30 && view(h, None)["sweep_margin"]["set"] == true);
        assert!(set_sweep_margin(h, "off", "t").unwrap().is_none() && sweep_margin(h).is_none());
        assert!(set_sweep_margin(h, "off", "t").unwrap().is_none(), "off twice");
        assert!(
            set_nas_room(&store::nasfs::NasRoot::local_for_test(h), "off", "t").unwrap().is_none()
                && room::nas_reserve(&store::nasfs::NasRoot::local_for_test(h)).is_none()
        );
        assert!(view(h, None)["nas_room"].is_null());
        let gone = h.join("nope");
        assert!(set_room(&gone, "2", "t").is_err() && set_memory(&gone, "2", "t").is_err());
        assert!(set_sweep_margin(&gone, "2", "t").is_err());
    }

    /// Task #927 review: the batch length is set from the build page as from `scenic pool set`: an
    /// ask of this Mac's agent for the lead (1 to 1440 minutes, or off), read back as the agent last
    /// had the pool's settings, the default until then.
    #[test]
    fn the_batch_length_is_asked_of_the_lead_and_read_back() {
        use crate::coord::asks::BATCH;
        let d = tempfile::tempdir().unwrap();
        let h = d.path();
        let v = view(h, None);
        assert_eq!((v["batch"]["minutes"].as_u64(), v["batch"]["had"].as_bool()), (Some(15), Some(false)));
        for bad in ["0", "1441", "ten", "-3"] {
            assert!(set_batch(h, bad, "t").is_err(), "{bad}");
        }
        assert!(set_batch(&h.join("nope"), "20", "t").is_err(), "no agent");
        let ask = |what: &str| {
            set_batch(h, what, "the build page").unwrap();
            let a = crate::control::take_setting_asks(h);
            assert!(a.len() == 1 && a[0].name == BATCH, "{a:?}");
            a[0].clone()
        };
        assert!(ask("off").value.is_null(), "off: the default again");
        let asked = [ask(" 20 ")];
        assert_eq!(asked[0].value, 20);
        let mut s = crate::coord::asks::Settings::default();
        assert!(s.take(&asked[0]));
        std::fs::write(h.join(crate::control::POOL_SETTINGS), serde_json::to_vec(&s).unwrap()).unwrap();
        let b = &view(h, None)["batch"];
        assert_eq!(
            (b["minutes"].as_u64(), b["set"].as_bool(), b["by"].as_str()),
            (Some(20), Some(true), Some("the build page"))
        );
    }
}
