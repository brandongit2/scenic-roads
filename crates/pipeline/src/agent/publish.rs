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
    match super::nasread::read_to_string(&root.join(switch)) {
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

/// Whether hand-offs of the map tiles or the road index waiting to be merged (`unmerged`) hold the
/// catalog back, a catalog held back `held_for` seconds so far: until build::CATALOG_CAP_S, then it
/// goes (the regions they touch not recorded built: build::plan's `ready`).
pub fn unmerged_holds(unmerged: bool, held_for: Option<u64>) -> bool {
    unmerged && held_for.is_none_or(|s| s < super::build::CATALOG_CAP_S)
}

/// When a catalog due was first held back (build::LastCatalog::held_at), now `now`, it last `was`:
/// held back now (`held`), kept (or now if none); else kept while a catalog is planned or due and
/// waiting out its minute (`due`: build::Plan::catalog_due), so the cap reached isn't lost; else
/// none, the count starting again.
pub fn held_at(was: Option<u64>, held: bool, due: bool, now: u64) -> Option<u64> {
    if held {
        was.or(Some(now))
    } else if due {
        was
    } else {
        None
    }
}

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
    fn unmerged_hand_offs_hold_the_catalog_up_to_the_cap() {
        let cap = super::super::build::CATALOG_CAP_S;
        assert!(unmerged_holds(true, None) && unmerged_holds(true, Some(cap - 1)));
        assert!(!unmerged_holds(true, Some(cap)) && !unmerged_holds(false, None));
    }

    #[test]
    fn the_cap_counts_from_the_first_hold_and_outlasts_the_minutes_wait() {
        assert_eq!(held_at(None, true, false, 100), Some(100));
        assert_eq!(held_at(Some(100), true, false, 700), Some(100));
        // Past the cap, a catalog due waiting out its minute: still counted.
        assert_eq!(held_at(Some(100), false, true, 720), Some(100));
        // None held and none due: the count starts again.
        assert_eq!(held_at(Some(100), false, false, 800), None);
    }
}
