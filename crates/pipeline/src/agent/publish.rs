//! Faster publishing (docs/plan.md §8, A map update; task #203): five switches under `state/pool/`, each
//! off while missing or `off` in it (the agent as it was, exactly), `on` in it (or present) on. They
//! act whatever the pool (map updates are the agent's), but `update-share` needs the lead's coordinator.
//! `update-per-region` and `update-share` were `round-per-region` and `round-share` before task #209:
//! read by whichever name was written last, the old copied to the new when only it is there
//! (`on_renamed`).
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
/// The two renamed switches' names before task #209 (a map update was a "round"): read while the
/// new name is missing, and copied to it once (`on_renamed`).
pub const OLD_UPDATE_PER_REGION: &str = "state/pool/round-per-region";
pub const OLD_UPDATE_SHARE: &str = "state/pool/round-share";

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

/// A renamed switch (`switch`, before task #209 `old`): while both names are there, by whichever
/// was written last (mtime; the new on a tie), so an edit under either name, by an app of either
/// age, is the one every app obeys; with only the old there, by it, copied to the new name as it
/// reads; with only the new, by it. The old is left for an older app until the cleanup. (To switch
/// one off, write `off` under either name; deleting one name leaves the other in force.)
fn on_renamed(root: &Path, switch: &str, old: &str) -> Option<bool> {
    // (A name's text and when it was written; None: not there; Err: not readable now.)
    let read = |name: &str| -> Result<Option<(String, std::time::SystemTime)>, ()> {
        match std::fs::read_to_string(root.join(name)) {
            Ok(s) => Ok(Some((s, std::fs::metadata(root.join(name)).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH)))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(()),
        }
    };
    let on = |s: &str| s.trim() != "off";
    match (read(switch).ok()?, read(old)) {
        (Some((n, nt)), Ok(Some((o, ot)))) => Some(on(if ot > nt { &o } else { &n })),
        (Some((n, _)), _) => Some(on(&n)),
        (None, Ok(Some((o, _)))) => {
            // (Not copied now, say the share is read-only here: read by the old name again next
            // time, the same.)
            let _ = crate::whole::write(&root.join(switch), o.as_bytes());
            Some(on(&o))
        }
        (None, Ok(None)) => Some(false),
        (None, Err(())) => None,
    }
}

impl Switches {
    /// The switches at `root`, each one that can't be read now as it was (`was`).
    pub fn read(root: &Path, was: Switches) -> Switches {
        Switches {
            per_region: on_renamed(root, UPDATE_PER_REGION, OLD_UPDATE_PER_REGION).unwrap_or(was.per_region),
            low_later: on(root, LOW_ZOOM_LATER).unwrap_or(was.low_later),
            share: on_renamed(root, UPDATE_SHARE, OLD_UPDATE_SHARE).unwrap_or(was.share),
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
    fn a_renamed_switch_is_read_by_whichever_name_was_written_last() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("state/pool")).unwrap();
        let read = |f: &str| std::fs::read_to_string(d.path().join(f)).ok();
        // (Each write at a time of its own, so the order doesn't hang on the clock's grain.)
        let put = |f: &str, s: &str, at: u64| {
            std::fs::write(d.path().join(f), s).unwrap();
            std::fs::File::options().write(true).open(d.path().join(f)).unwrap().set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(at)).unwrap();
        };
        // Only the old names (as an older app knew them): read, and copied to the new; the old kept.
        put(OLD_UPDATE_PER_REGION, "on\n", 1000);
        put(OLD_UPDATE_SHARE, "off\n", 1000);
        let s = Switches::read(d.path(), Switches::default());
        assert!(s.per_region && !s.share);
        assert_eq!((read(UPDATE_PER_REGION), read(UPDATE_SHARE)), (Some("on\n".into()), Some("off\n".into())));
        assert_eq!((read(OLD_UPDATE_PER_REGION), read(OLD_UPDATE_SHARE)), (Some("on\n".into()), Some("off\n".into())));
        // Both there: the one written last, under either name (an older app's edit obeyed too).
        put(UPDATE_SHARE, "off", 2000);
        put(OLD_UPDATE_SHARE, "on", 3000);
        assert!(Switches::read(d.path(), Switches::default()).share);
        put(UPDATE_SHARE, "off", 4000);
        assert!(!Switches::read(d.path(), Switches::default()).share);
        // (A tie: the new.)
        put(OLD_UPDATE_SHARE, "on", 4000);
        assert!(!Switches::read(d.path(), Switches::default()).share);
        // Neither name: off. The new name unreadable (a folder): as it was.
        std::fs::remove_file(d.path().join(OLD_UPDATE_PER_REGION)).unwrap();
        std::fs::remove_file(d.path().join(UPDATE_PER_REGION)).unwrap();
        assert!(!Switches::read(d.path(), Switches { per_region: true, ..Default::default() }).per_region);
        std::fs::create_dir_all(d.path().join(UPDATE_PER_REGION)).unwrap();
        put(OLD_UPDATE_PER_REGION, "off", 5000);
        assert!(Switches::read(d.path(), Switches { per_region: true, ..Default::default() }).per_region);
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
