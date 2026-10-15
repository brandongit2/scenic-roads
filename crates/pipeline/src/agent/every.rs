//! Every step to every member (docs/pool.md §7.2, §7.7; §12 phase 4's ninth batch), behind the
//! switch `state/pool/every-step` (`SWITCH`; off while missing or `off` in it: the agent as it was,
//! exactly). It acts only in the pool.
//!
//! - **Every step of the plan offered** (`offered`): the lead's coordinator offers its members the
//!   plan's steps, not only the shared ones (the map tiles' `pack` and `lo` among them; each Mac's
//!   saves reach the records only through the journal, so a member's are merged as any). Kept to
//!   the lead: its duties (`LEAD_ONLY`: the catalogs, a prune, the 3D buildings' sources' fetch),
//!   the steps that keep the pass's answers (the agent seeds them as it starts), and the worldwide
//!   steps (one target, the world: the roads' index, the rail stops, the ferries, the world's roots,
//!   the landmarks' and overlays' draws, the rail chain, the terrain's water; the pass's own), whose
//!   inputs this batch doesn't follow onto a member's Mac.
//! - **By the measures** (crate::coord's `pick_every`): a member offered a target only when its
//!   measured memory and disk fit what the member spares; a cold start (a step none of whose
//!   targets has run) only to the member with the most room, the lead's own counted, alone; a job's
//!   targets as many as fit a quarter of an hour by their measured times (crate::coord::JOB_S); a
//!   contiguous run of the plan, from its front or just after the member's last.
//! - **Claims no longer read** (crate::agent::claims): with the switch on in the pool, the agent
//!   neither claims nor reads others' claims; what another member builds is the lead's
//!   coordinator's leases alone.

use std::path::Path;

/// The switch (`on` in it, or present; `off` in it, or missing: off, the agent as it was).
pub const SWITCH: &str = "state/pool/every-step";

/// Whether every step to every member is on, as the switch at `root` says; None when it can't be
/// read now.
pub fn on(root: &Path) -> Option<bool> {
    match std::fs::read_to_string(root.join(SWITCH)) {
        Ok(s) => Some(s.trim() != "off"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(false),
        Err(_) => None,
    }
}

/// The steps kept to the lead whatever the switch: the round's catalogs and prune (its gates:
/// crate::agent's catalog and sweep), the 3D buildings' sources' fetch (written outside the records,
/// under `sources/`), and its duties outside the plan (the OSM pass, offered only to the Mac holding
/// its progress: crate::agent::resume; GC, the backup, the input checks, the names to do).
pub const LEAD_ONLY: [&str; 9] = ["catalog", "catalog-held", "prune", "bld-fetch", "osm-pass", "gc", "backup", "inputs", "names-todo"];

/// Whether the lead offers its members `step`'s targets with the switch on: a step of the steps
/// table of targets by area (not a worldwide one: `names_target` of its name, a run alone, the
/// spoken languages) that isn't the lead's (`LEAD_ONLY`) and doesn't keep the pass's answers
/// (crate::agent::steps::ANSWERED), the shared steps among them.
pub fn offered(step: &str) -> bool {
    super::steps::row(step).is_some() && !LEAD_ONLY.contains(&step) && !super::steps::ANSWERED.contains(&step) && names_target(step) && !super::steps::alone(step) && step != "spoken"
}

/// Whether a job's command names target `t` (a step's worldwide target, its own name or a world
/// root, isn't named: its step's job does the whole world), the lead's own jobs' and a member's
/// alike.
pub fn names_target(t: &str) -> bool {
    !matches!(t, "catalog" | "items" | "marks" | "roadunits" | "stations" | "ferries" | "heritage-sites" | "heritage" | "overlays" | "rail-feeds" | "rail" | "bld-fetch" | "terrain-water") && !t.ends_with("-root")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_switch_is_off_while_missing_or_off() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(on(d.path()), Some(false));
        std::fs::create_dir_all(d.path().join("state/pool")).unwrap();
        std::fs::write(d.path().join(SWITCH), "off\n").unwrap();
        assert_eq!(on(d.path()), Some(false));
        std::fs::write(d.path().join(SWITCH), "on\n").unwrap();
        assert_eq!(on(d.path()), Some(true));
        std::fs::write(d.path().join(SWITCH), "").unwrap();
        assert_eq!(on(d.path()), Some(true));
    }

    #[test]
    fn every_step_but_the_leads_and_the_answers_is_offered() {
        for s in ["pack", "lo", "terrain-lo", "slope-lo", "trees-lo", "bldfield-lo"] {
            assert!(offered(s), "{s}");
        }
        for s in super::super::steps::SHARED {
            assert!(offered(s), "{s}");
        }
        for s in ["catalog", "catalog-held", "prune", "bld-fetch", "osm-pass", "gc", "inputs", "items", "heritage", "heritage-sites", "tail", "roadunits", "stations", "ferries", "terrain-root", "slope-root", "marks", "overlays", "rail", "rail-feeds", "terrain-water", "water", "labels", "spoken", "a step of a newer app"] {
            assert!(!offered(s), "{s}");
        }
    }

    #[test]
    fn worldwide_targets_are_not_named() {
        assert!(names_target("6/1/2") && names_target("3/2/2"));
        assert!(!names_target("roadunits") && !names_target("terrain-root") && !names_target("slope-root"));
    }
}
