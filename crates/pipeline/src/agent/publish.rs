//! Publishing's switches (docs/plan.md §8, Publishing): three under `state/pool/`, each off while
//! missing or `off` in it, `on` in it (or present) on. They act whatever the pool (publishing is
//! the lead's agent's).
//!
//! - **The low zooms later** (`low-zoom-later`): the roads' and rails' lo packs (z3–8) and the
//!   terrain's and slope's roots (z0–2) only `LOW_EVERY_S` after they were last made, once half
//!   the lo packs are stale, and once everything is built (crate::agent::build::Publishing); the
//!   map tiles (z9 up) as soon as their owners change.
//! - **Delta catalogs** (`delta-catalog`): the catalog written as what changed over the last
//!   (store::catalog::Delta; a whole one once the deltas since the last whole one add up to one),
//!   and the sizes of the files the last one lists taken from it, not looked at again.
//! - **Slope and tree cover near** (`near-slope`): a done region's slope and tree cover the pieces
//!   near it (crate::agent::build::Publishing::near), their assemblies with the low zooms.
//!
//! The lead's own job of the map tiles (`pack`, `lo`) takes its share of them (`lead_share`), the
//! rest left to the members asking (every step to every member: crate::agent::every), not the
//! step's whole batch.

use std::path::Path;

pub const LOW_ZOOM_LATER: &str = "state/pool/low-zoom-later";
pub const DELTA_CATALOG: &str = "state/pool/delta-catalog";
pub const NEAR_SLOPE: &str = "state/pool/near-slope";
/// The switches as last read.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Switches {
    pub low_later: bool,
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
            low_later: on(root, LOW_ZOOM_LATER).unwrap_or(was.low_later),
            delta: on(root, DELTA_CATALOG).unwrap_or(was.delta),
            near: on(root, NEAR_SLOPE).unwrap_or(was.near),
        }
    }
}

/// How many of `total` targets of a map tiles' step (`pack`, `lo`) the lead's own job takes, with
/// `members` other members asking for work: an even share (`total / (members + 1)`, rounded up),
/// at most the step's batch; the step's batch with none.
pub fn lead_share(total: usize, batch: usize, members: usize) -> usize {
    if members == 0 {
        return batch;
    }
    total.div_ceil(members + 1).clamp(1, batch.max(1))
}

/// The steps the lead shares with the members (`lead_share`).
pub const SHARED_MAP_TILES: [&str; 2] = ["pack", "lo"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_switch_is_off_while_missing_or_off_and_kept_while_unreadable() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(Switches::read(d.path(), Switches::default()), Switches::default());
        std::fs::create_dir_all(d.path().join("state/pool")).unwrap();
        for (f, s) in [(LOW_ZOOM_LATER, ""), (DELTA_CATALOG, "on\n"), (NEAR_SLOPE, "off\n")] {
            std::fs::write(d.path().join(f), s).unwrap();
        }
        let s = Switches::read(d.path(), Switches::default());
        assert_eq!(s, Switches { low_later: true, delta: true, near: false });
        // (A switch that's a folder can't be read: as it was.)
        std::fs::remove_file(d.path().join(NEAR_SLOPE)).unwrap();
        std::fs::create_dir_all(d.path().join(NEAR_SLOPE)).unwrap();
        let was = Switches { near: true, ..Default::default() };
        assert!(Switches::read(d.path(), was).near);
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
