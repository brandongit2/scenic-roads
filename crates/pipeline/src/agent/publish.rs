//! Faster publishing (docs/plan.md §8, A map update; task #203): five switches under `state/pool/`, each
//! off while missing or `off` in it (the agent as it was, exactly), `on` in it (or present) on. They
//! act whatever the pool (map updates are the agent's), but `update-share` needs the lead's coordinator.
//!
//! - **A map update per region** (`update-per-region`): a map update for a region done as soon as the last
//!   map update's time allows (`update_gap`: twice the map updates' measured time, so a map update begins at least
//!   one map update's length after the last ended and the regions done meanwhile go out together), not
//!   an hour after the last began.
//! - **The low zooms later** (`low-zoom-later`): the roads' and rails' lo packs (z3–8) and the
//!   terrain's and slope's roots (z0–2) only in a map update that makes the low zooms
//!   (crate::agent::build::MapUpdate::low: `LOW_EVERY_S` after the last that did, once half the lo
//!   packs are stale, and the last map update); the map tiles (z9 up) in every map update.
//! - **A map update shared** (`update-share`): the lead's own job of a map update's map tiles (`pack`, `lo`)
//!   takes its share of them (`lead_share`), the rest left to the members asking (every step to
//!   every member: crate::agent::every), not the step's whole batch.
//! - **Delta catalogs** (`delta-catalog`): the catalog written as what changed over the last
//!   (store::catalog::Delta; a whole one once the deltas since the last whole one add up to one),
//!   and the sizes of the files the last one lists taken from it, not looked at again.
//! - **Slope and tree cover near** (`near-slope`): a map update's slope and tree cover the pieces near
//!   its regions (crate::agent::build::Publishing::near), their assemblies with the low zooms.

use std::path::Path;

pub const UPDATE_PER_REGION: &str = "state/pool/update-per-region";
pub const LOW_ZOOM_LATER: &str = "state/pool/low-zoom-later";
pub const UPDATE_SHARE: &str = "state/pool/update-share";
pub const DELTA_CATALOG: &str = "state/pool/delta-catalog";
pub const NEAR_SLOPE: &str = "state/pool/near-slope";
/// The switches as last read.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Switches {
    pub per_region: bool,
    pub low_later: bool,
    pub share: bool,
    pub delta: bool,
    pub near: bool,
}

/// Whether the switch at `root` is on; None when it can't be read now.
fn on(root: &Path, switch: &str) -> Option<bool> {
    match std::fs::read_to_string(root.join(switch)) {
        Ok(s) => Some(s.trim() != "off"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(false),
        Err(_) => None,
    }
}

impl Switches {
    /// The switches at `root`, each one that can't be read now as it was (`was`).
    pub fn read(root: &Path, was: Switches) -> Switches {
        Switches {
            per_region: on(root, UPDATE_PER_REGION).unwrap_or(was.per_region),
            low_later: on(root, LOW_ZOOM_LATER).unwrap_or(was.low_later),
            share: on(root, UPDATE_SHARE).unwrap_or(was.share),
            delta: on(root, DELTA_CATALOG).unwrap_or(was.delta),
            near: on(root, NEAR_SLOPE).unwrap_or(was.near),
        }
    }
}

/// How long after the last map update began a region's map update may begin, a map update per region: twice the
/// map updates' measured time (crate::agent::forecast::update_secs: the last five map updates' chains and
/// catalogs), so the next begins at least one map update's length after the last ended (at most half the
/// time on map updates, the regions done meanwhile together), and never more than the hour it was
/// (build::PUBLISH_EVERY_S). None measured yet: the hour.
pub fn update_gap(measured_s: Option<f64>) -> u64 {
    match measured_s.filter(|s| s.is_finite() && *s >= 0.0) {
        Some(s) => ((2.0 * s).ceil() as u64).min(super::build::PUBLISH_EVERY_S),
        None => super::build::PUBLISH_EVERY_S,
    }
}

/// How many of a map update's `total` targets of a map tiles' step (`pack`, `lo`) the lead's own job
/// takes, a map update shared, with `members` other members asking for work: an even share
/// (`total / (members + 1)`, rounded up), at most the step's batch; the step's batch with none.
pub fn lead_share(total: usize, batch: usize, members: usize) -> usize {
    if members == 0 {
        return batch;
    }
    total.div_ceil(members + 1).clamp(1, batch.max(1))
}

/// The steps a map update shared splits with the members (`lead_share`).
pub const SHARED_IN_UPDATE: [&str; 2] = ["pack", "lo"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_switch_is_off_while_missing_or_off_and_kept_while_unreadable() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(Switches::read(d.path(), Switches::default()), Switches::default());
        std::fs::create_dir_all(d.path().join("state/pool")).unwrap();
        for (f, s) in [(UPDATE_PER_REGION, "on"), (LOW_ZOOM_LATER, ""), (UPDATE_SHARE, "off\n"), (DELTA_CATALOG, "on\n")] {
            std::fs::write(d.path().join(f), s).unwrap();
        }
        let s = Switches::read(d.path(), Switches::default());
        assert_eq!(s, Switches { per_region: true, low_later: true, share: false, delta: true, near: false });
        // (A switch that's a folder can't be read: as it was.)
        std::fs::create_dir_all(d.path().join(NEAR_SLOPE)).unwrap();
        let was = Switches { near: true, ..Default::default() };
        assert!(Switches::read(d.path(), was).near);
    }

    #[test]
    fn the_gap_is_twice_the_measured_map_updates_at_most_the_hour() {
        assert_eq!(update_gap(None), 3600);
        assert_eq!(update_gap(Some(150.0)), 300);
        assert_eq!(update_gap(Some(150.2)), 301);
        assert_eq!(update_gap(Some(5000.0)), 3600);
        assert_eq!(update_gap(Some(f64::NAN)), 3600);
    }

    #[test]
    fn the_lead_takes_its_share() {
        assert_eq!(lead_share(10, 16, 0), 16);
        assert_eq!(lead_share(10, 16, 1), 5);
        assert_eq!(lead_share(11, 16, 1), 6);
        assert_eq!(lead_share(40, 16, 1), 16);
        assert_eq!(lead_share(1, 16, 3), 1);
        assert_eq!(lead_share(3, 2, 1), 2);
    }
}
