//! A job far over its memory (docs/pool.md §7.2): macOS swaps rather than kills, so a job holding
//! more than its Mac has leaves the Mac swapping for as long as it runs, every other job and the
//! owner's own work with it. The agent samples each running job's memory every loop (its processes'
//! physical footprints summed: crate::sys::footprint_of_group) and keeps, per target, the most a job
//! held while that target was the one under way (`current`): what it learns (a target's **floor**,
//! the least it takes, crate::coord's `floors`) whatever the guard does. With the guard on (`on`,
//! the switch `SWITCH`), the jobs holding more together than the Mac's limit (`limit_mb`) are
//! stopped by measure alone (`decide`), no prediction trusted: the job beside the largest, at its
//! next safe point, when the largest fits alone; else the largest, at once, given back, not failed.
//! A target whose floor passes this Mac's limit isn't started here again; one past every Mac's in the
//! pool waits, the status saying why.

use std::collections::BTreeMap;
use std::path::Path;

/// The switch, on the NAS: `off` in it turns the guard off, `on` on; missing, the guard is as
/// `DEFAULT` says.
pub const SWITCH: &str = "state/pool/memory-guard";

/// Whether the guard stops jobs when its switch is missing (the owner's choice: on).
pub const DEFAULT: bool = true;

/// The guard as the switch at `root` says: on or off; None when it can't be read now (the share not
/// answering: the caller keeps what it had).
pub fn on(root: &Path) -> Option<bool> {
    match std::fs::read_to_string(root.join(SWITCH)) {
        Ok(s) => Some(match s.trim() {
            "off" => false,
            "on" => true,
            // (Anything else: as if missing, said once by the caller's status.)
            _ => DEFAULT,
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(DEFAULT),
        Err(_) => None,
    }
}

/// The most memory the jobs on a Mac of `total_mb` may hold together (MB): its memory less an eighth
/// for macOS and the owner's own work, 4 GB at least (42 GB of the M4's 48, 12 of the M1's 16).
pub fn limit_mb(total_mb: u64) -> u64 {
    total_mb.saturating_sub((total_mb / 8).max(4096))
}

/// What the guard does about the jobs in a Mac's slots, by what each holds now (MB; None: no job in
/// that slot, or its memory unknown).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// Nothing: the jobs fit the limit together.
    Nothing,
    /// Slot k's job stops at its next safe point: the job beside it fits the limit alone.
    Drain(usize),
    /// Slot k's job stops at once: it holds more than the limit by itself.
    Stop(usize),
}

/// The guard's decision (`Act`), by measure alone: nothing while the jobs together fit `limit`;
/// else the largest stopped at once when it alone passes it, or the others drained (each in turn:
/// the first not draining yet, `draining`) when it fits alone.
pub fn decide(held: &[Option<u64>], limit: u64, draining: &[bool]) -> Act {
    let total: u64 = held.iter().flatten().sum();
    if total <= limit {
        return Act::Nothing;
    }
    let Some((big, mb)) = held.iter().enumerate().filter_map(|(k, m)| m.map(|m| (k, m))).max_by_key(|&(k, m)| (m, std::cmp::Reverse(k))) else { return Act::Nothing };
    if mb > limit {
        return Act::Stop(big);
    }
    match held.iter().enumerate().find(|&(k, m)| k != big && m.is_some() && !draining.get(k).copied().unwrap_or(false)) {
        Some((k, _)) => Act::Drain(k),
        None => Act::Nothing,
    }
}

/// The target a job is on, as its costs file says (`SCENIC_COSTS`: a `started` line as each target
/// begins, a `peak_mb` line as it ends): the last begun and not ended, by its cost key
/// (crate::coord::cost_key); None when the file says none (a step that notes no costs, or between
/// targets).
pub fn current(costs: &Path) -> Option<String> {
    let s = std::fs::read_to_string(costs).ok()?;
    let mut on: Option<String> = None;
    for l in s.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(l) else { continue };
        let Some(key) = v["unit"].as_str() else { continue };
        if v.get("started").is_some() {
            on = Some(key.to_string());
        } else if v.get("peak_mb").is_some() && on.as_deref() == Some(key) {
            on = None;
        }
    }
    on
}

/// What a job's processes held beyond its target's own measure (MB): a unit's measure is its
/// programs' most, one at a time, and the job holds a GB besides (as the agent reckons a unit job's
/// memory); other steps measure the job's processes together.
pub fn own_mb(step: &str) -> u64 {
    if step == "unit" { 1024 } else { 0 }
}

/// The floors a job's run teaches (cost key → MB), from the most it held while each target was under
/// way (`seen`, the group's footprint), less what the job holds beyond its target (`own_mb`), but
/// for the targets its run measured itself (`measured`: their measure is better, sampled four times
/// a second from the target's start).
pub fn floors(step: &str, seen: &BTreeMap<String, u64>, measured: &[String]) -> Vec<(String, u64)> {
    seen.iter().filter(|(k, _)| !measured.contains(k)).map(|(k, mb)| (k.clone(), mb.saturating_sub(own_mb(step)))).filter(|(_, mb)| *mb > 0).collect()
}

/// The guard in the status: its switch, this Mac's limit, what the jobs hold now, and what it last
/// did (when, unix seconds, and in words).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct View {
    pub on: bool,
    pub limit_mb: u64,
    pub held_mb: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<(u64, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limit_is_the_macs_memory_less_an_eighth() {
        assert_eq!(limit_mb(48 << 10), 42 << 10);
        assert_eq!(limit_mb(16 << 10), 12 << 10);
        assert_eq!(limit_mb(64 << 10), 56 << 10);
        assert_eq!(limit_mb(2048), 0);
    }

    #[test]
    fn decisions_by_measure_alone() {
        let l = 42 << 10;
        // Together within the limit: nothing, however far over what was expected.
        assert_eq!(decide(&[Some(30 << 10), Some(12 << 10)], l, &[false, false]), Act::Nothing);
        assert_eq!(decide(&[None, None], l, &[false, false]), Act::Nothing);
        // Over together, the largest fitting alone: the one beside it drained, once.
        assert_eq!(decide(&[Some(35 << 10), Some(10 << 10)], l, &[false, false]), Act::Drain(1));
        assert_eq!(decide(&[Some(10 << 10), Some(35 << 10)], l, &[false, false]), Act::Drain(0));
        assert_eq!(decide(&[Some(35 << 10), Some(10 << 10)], l, &[false, true]), Act::Nothing);
        // The largest past the limit alone: stopped at once, draining or not beside it.
        assert_eq!(decide(&[Some(43 << 10), Some(1 << 10)], l, &[false, true]), Act::Stop(0));
        assert_eq!(decide(&[None, Some(50 << 10)], l, &[false, false]), Act::Stop(1));
        assert_eq!(decide(&[Some(50 << 10)], l, &[false]), Act::Stop(0));
        // (Two the same: the first slot's counts as the largest, the second drained.)
        assert_eq!(decide(&[Some(25 << 10), Some(25 << 10)], l, &[false, false]), Act::Drain(1));
    }

    #[test]
    fn the_target_under_way_is_the_last_begun_and_not_ended() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("costs.jsonl");
        assert_eq!(current(&p), None);
        std::fs::write(&p, "{\"unit\":\"terrain 6/1/1\",\"started\":1}\n").unwrap();
        assert_eq!(current(&p).as_deref(), Some("terrain 6/1/1"));
        std::fs::write(&p, "{\"unit\":\"terrain 6/1/1\",\"started\":1}\n{\"unit\":\"terrain 6/1/1\",\"peak_mb\":900,\"secs\":3}\n").unwrap();
        assert_eq!(current(&p), None);
        std::fs::write(&p, "{\"unit\":\"terrain 6/1/1\",\"started\":1}\n{\"unit\":\"terrain 6/1/1\",\"peak_mb\":900,\"secs\":3}\n{\"unit\":\"terrain 6/1/2\",\"started\":4}\nnot json\n").unwrap();
        assert_eq!(current(&p).as_deref(), Some("terrain 6/1/2"));
    }

    #[test]
    fn floors_from_what_a_run_held_but_what_it_measured() {
        let seen: BTreeMap<String, u64> = [("6/1/1".to_string(), 9000), ("6/1/2".to_string(), 7000), ("6/1/3".to_string(), 500)].into();
        assert_eq!(floors("unit", &seen, &["6/1/2".to_string()]), [("6/1/1".to_string(), 7976)]);
        let seen: BTreeMap<String, u64> = [("water water".to_string(), 33000)].into();
        assert_eq!(floors("water", &seen, &[]), [("water water".to_string(), 33000)]);
    }

    #[test]
    fn the_switch() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(on(d.path()), Some(DEFAULT));
        std::fs::create_dir_all(d.path().join("state/pool")).unwrap();
        std::fs::write(d.path().join(SWITCH), "off\n").unwrap();
        assert_eq!(on(d.path()), Some(false));
        std::fs::write(d.path().join(SWITCH), "on").unwrap();
        assert_eq!(on(d.path()), Some(true));
    }
}
