//! The owner's settings (docs/coordinator.md §7): this Mac's in-use share (crate::agent::share), memory
//! floor (crate::agent::memfloor) and disk room target (crate::agent::room::Target), the pool's
//! NAS reserve (buildstore::room::NAS_RESERVE), batch length and overdue factor (the lead's
//! settings `batch-minutes` and `overdue-factor`, crate::coord::asks), the dev sweep's margin
//! (`SWEEP_MARGIN`), and the NAS calls a process of this Mac holds past their deadline
//! (store::deadline's cap, `set_nas_held`). Each is set through one function here, whoever asks: `scenic in-use`, `scenic
//! memory`, `scenic room`, `scenic nas-room`, `scenic pool set batch-minutes|overdue-factor`, and the
//! build page's
//! POST /api/settings (crate::coord), which reads them back with `view` (the sweep's margin: the
//! build page alone). The map server's mirror reads the room target through buildstore::room's
//! `RoomTarget` (its reserve is the larger of the two).

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
    let total_mb = jobrt::sys::memsize().map_or(0, |b| b >> 20);
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

/// The most NAS calls a process may hold past their deadline (`set_nas_held`).
pub const NAS_HELD_MAX: usize = 1000;

/// Sets this Mac's cap on the NAS calls a process holds in the kernel past their deadline
/// (store::deadline: its file in `home`, read by its agent each round and given to each job as it
/// starts, read by the CLI as it starts and followed by the map server): `what` calls (1 to `NAS_HELD_MAX`), or "off": the default again
/// (store::deadline::CAP_DEFAULT).
pub fn set_nas_held(home: &Path, what: &str, by: &str) -> Result<Option<usize>> {
    let p = home.join(store::deadline::CAP_FILE);
    if what == "off" {
        match std::fs::remove_file(&p) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => return Ok(None),
        }
    }
    let n: usize = what
        .trim()
        .parse()
        .ok()
        .filter(|n| (1..=NAS_HELD_MAX).contains(n))
        .with_context(|| format!("the NAS calls held: 1 to {NAS_HELD_MAX} calls or off, not {what}"))?;
    anyhow::ensure!(home.is_dir(), "no agent here ({} isn't there)", home.display());
    let v = serde_json::json!({ "n": n, "by": by, "at": super::jobs::now_s() });
    store::whole::write(&p, &serde_json::to_vec(&v)?)?;
    Ok(Some(n))
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
/// with its next beat (jobrt::control::request_setting), as every pool setting is.
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
    jobrt::control::request_setting(home, &a)
}

/// The pool's batch length as this Mac's agent last had the pool's settings from the lead (in
/// `home`; `had` false: not yet), in minutes.
fn batch_view(home: &Path) -> serde_json::Value {
    let s = jobrt::control::read_pool_settings(home);
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
    jobrt::control::request_setting(home, &ask)
}

/// The pool's overdue factor as this Mac's agent last had the pool's settings from the lead (in
/// `home`; `had` false: not yet).
fn overdue_view(home: &Path) -> serde_json::Value {
    let s = jobrt::control::read_pool_settings(home);
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

/// Asks one of the pool's switches (`asks::SWITCHES`: `hold-catalog`, `near-slope`, `placement`)
/// of this Mac's agent in `home`, which sends it to the lead with its next beat (as `scenic pool
/// set <name> on|off`): `what` "on" or "off".
pub fn set_switch(home: &Path, name: &str, what: &str, by: &str) -> Result<()> {
    use crate::coord::asks::SWITCHES;
    anyhow::ensure!(SWITCHES.contains(&name), "{name}: not one of the pool's switches ({})", SWITCHES.join(", "));
    let value = match what.trim() {
        "on" => true,
        "off" => false,
        _ => anyhow::bail!("{name}: on or off, not {what}"),
    };
    anyhow::ensure!(home.is_dir(), "no agent here ({} isn't there)", home.display());
    let ask = crate::coord::asks::SettingAsk {
        name: name.into(),
        value: value.into(),
        by: by.to_string(),
        at: super::jobs::now_s(),
    };
    jobrt::control::request_setting(home, &ask)
}

/// A switch of the pool's (`name`) as this Mac's agent last had the pool's settings from the lead
/// (in `home`; `had` false: not yet).
fn switch_view(home: &Path, name: &str) -> serde_json::Value {
    let s = jobrt::control::read_pool_settings(home);
    let v = s.as_ref().and_then(|s| s.values.get(name)).filter(|v| !v.value.is_null());
    serde_json::json!({
        "on": s.as_ref().is_some_and(|s| s.on(name)),
        "set": v.is_some(),
        "by": v.map(|v| v.by.clone()),
        "at": v.map(|v| v.at),
        "had": s.is_some(),
    })
}

/// The settings as they stand, for the build page: this Mac's (in its agent's folder `home`), the
/// pool's NAS reserve (under `root`; null while the NAS is away), and its batch length and overdue
/// factor (as this Mac's agent last had them from the lead). Sizes in bytes, memory in MB.
pub fn view(home: &Path, root: Option<&store::nasfs::NasRoot>) -> serde_json::Value {
    let total_mb = jobrt::sys::memsize().map_or(0, |b| b >> 20);
    let mem = memfloor::read(home);
    let sh = share::read(home);
    let cores = cores();
    let margin = sweep_margin(home);
    let held: Option<serde_json::Value> = std::fs::read(home.join(store::deadline::CAP_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .filter(|_| store::deadline::cap_in(home).is_some());
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
        "nas_held": {
            "n": store::deadline::cap_in(home).unwrap_or(store::deadline::CAP_DEFAULT),
            "default": store::deadline::CAP_DEFAULT,
            "max": NAS_HELD_MAX,
            "set": held.is_some(),
            "by": held.as_ref().map(|h| h["by"].clone()),
            "at": held.as_ref().map(|h| h["at"].clone()),
        },
        "nas_room": root.map(|r| target_view(room::nas_reserve(r), room::nas_free(r).ok(), None)),
        "batch": batch_view(home),
        "overdue": overdue_view(home),
        "hold_catalog": switch_view(home, crate::coord::asks::HOLD_CATALOG),
        "near_slope": switch_view(home, crate::coord::asks::NEAR_SLOPE),
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
            assert!(set_sweep_margin(h, bad, "t").is_err(), "{bad}");
            assert!(set_nas_room(&store::nasfs::NasRoot::local_for_test(h), bad, "t").is_err(), "{bad}");
        }
        assert!(set_memory(h, "1000000", "t").is_err(), "more than this Mac has");
        let all = (jobrt::sys::memsize().unwrap() >> 30).to_string();
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

    /// Task #1141: the NAS calls held, set from the build page: 1 to `NAS_HELD_MAX` or off, read
    /// back by store::deadline (what the agent applies and gives its jobs) and in the view.
    #[test]
    fn the_nas_calls_held_are_set_and_read_back() {
        let d = tempfile::tempdir().unwrap();
        let h = d.path();
        let v = view(h, None);
        assert_eq!(
            (v["nas_held"]["n"].as_u64(), v["nas_held"]["set"].as_bool()),
            (Some(store::deadline::CAP_DEFAULT as u64), Some(false))
        );
        for bad in ["0", "1001", "x", "-3", "2.5"] {
            assert!(set_nas_held(h, bad, "t").is_err(), "{bad}");
        }
        assert!(set_nas_held(&h.join("nope"), "8", "t").is_err(), "no agent");
        assert_eq!(set_nas_held(h, " 8 ", "the build page").unwrap(), Some(8));
        assert_eq!(store::deadline::cap_in(h), Some(8));
        let v = view(h, None);
        assert_eq!(
            (v["nas_held"]["n"].as_u64(), v["nas_held"]["set"].as_bool(), v["nas_held"]["by"].as_str()),
            (Some(8), Some(true), Some("the build page"))
        );
        assert_eq!(set_nas_held(h, "off", "t").unwrap(), None);
        assert_eq!(store::deadline::cap_in(h), None);
        assert!(set_nas_held(h, "off", "t").unwrap().is_none(), "off twice");
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
            let a = jobrt::control::take_setting_asks(h);
            assert!(a.len() == 1 && a[0].name == BATCH, "{a:?}");
            a[0].clone()
        };
        assert!(ask("off").value.is_null(), "off: the default again");
        let asked = [ask(" 20 ")];
        assert_eq!(asked[0].value, 20);
        let mut s = crate::coord::asks::Settings::default();
        assert!(s.take(&asked[0]));
        std::fs::write(h.join(jobrt::control::POOL_SETTINGS), serde_json::to_vec(&s).unwrap()).unwrap();
        let b = &view(h, None)["batch"];
        assert_eq!(
            (b["minutes"].as_u64(), b["set"].as_bool(), b["by"].as_str()),
            (Some(20), Some(true), Some("the build page"))
        );
    }

    /// Task #1119: publishing's switches (once files on the NAS) are the pool's settings, set from
    /// the build page as from `scenic pool set`: on or off, asked of the lead, read back as the
    /// agent last had them, off until then.
    #[test]
    fn publishings_switches_are_asked_of_the_lead_and_read_back() {
        use crate::coord::asks::{HOLD_CATALOG, NEAR_SLOPE};
        let d = tempfile::tempdir().unwrap();
        let h = d.path();
        let v = view(h, None);
        assert_eq!((v["hold_catalog"]["on"].as_bool(), v["hold_catalog"]["had"].as_bool()), (Some(false), Some(false)));
        assert!(set_switch(h, HOLD_CATALOG, "maybe", "t").is_err());
        assert!(set_switch(h, "batch-minutes", "on", "t").is_err(), "not a switch");
        assert!(set_switch(&h.join("nope"), NEAR_SLOPE, "on", "t").is_err(), "no agent");
        set_switch(h, HOLD_CATALOG, "on", "the build page").unwrap();
        set_switch(h, NEAR_SLOPE, " off ", "the build page").unwrap();
        let asked = jobrt::control::take_setting_asks(h);
        assert_eq!(
            asked.iter().map(|a| (a.name.as_str(), a.value.clone())).collect::<Vec<_>>(),
            [(HOLD_CATALOG, true.into()), (NEAR_SLOPE, false.into())]
        );
        let mut s = crate::coord::asks::Settings::default();
        for a in &asked {
            assert!(s.take(a));
        }
        std::fs::write(h.join(jobrt::control::POOL_SETTINGS), serde_json::to_vec(&s).unwrap()).unwrap();
        let v = view(h, None);
        assert_eq!(
            (v["hold_catalog"]["on"].as_bool(), v["hold_catalog"]["by"].as_str(), v["near_slope"]["on"].as_bool()),
            (Some(true), Some("the build page"), Some(false))
        );
    }
}
