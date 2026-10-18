//! Every step to every member (docs/pool.md §7.2, §7.7; §12 phase 4's ninth batch), always on in
//! the pool (§12 phase 4's thirteenth batch: `state/pool/every-step`, its switch of before, is no
//! longer read).
//!
//! - **Every step of the plan offered** (`offered`): the lead's coordinator offers its members the
//!   plan's steps, not only the shared ones (the map tiles' `pack` and `lo` among them; each Mac's
//!   saves reach the records only through the journal, so a member's are merged as any). Kept to
//!   the lead: its duties (`LEAD_ONLY`: the catalogs, a prune, the 3D buildings' sources' fetch),
//!   the steps that keep the pass's answers (the agent seeds them as it starts), and the worldwide
//!   steps (one target, the world: the roads' index, the rail stops, the ferries, the world's roots,
//!   the landmarks' and overlays' draws, the rail chain, the terrain's water; the pass's own), whose
//!   inputs aren't followed onto a member's Mac.
//! - **By the measures** (crate::coord's `pick_every`): a member offered a target only when its
//!   measured memory and disk fit what the member spares; a cold start (a step none of whose
//!   targets has run) only to the member with the most room, the lead's own counted, alone; a job's
//!   targets as many as fit a quarter of an hour by their measured times (crate::coord::JOB_S); a
//!   contiguous run of the plan, from its front or just after the member's last.
//! - What another member builds is the lead's coordinator's leases alone.
//! - **Every job by grant** (`grants`): the lead's own agent asks its coordinator for work as any
//!   member does (crate::coord::Coordinator::ask_own), its jobs packed by the same measures. The
//!   steps kept to the lead (`LEAD_ONLY`, the answer-keeping steps, the worldwide steps: those not
//!   `offered`) and the lead's own planned jobs are granted to the current lead's member alone.

/// The steps kept to the lead: the catalogs and prune (their gates:
/// crate::agent's catalog and sweep), the 3D buildings' sources' fetch (written outside the records,
/// under `sources/`), and its duties outside the plan (the OSM pass, offered only to the Mac holding
/// its progress: crate::agent::resume; GC, the backup, the input checks, the names and timetables to do: written into the drop boxes'
/// `todo/`, outside the records).
pub const LEAD_ONLY: [&str; 11] = [
    "catalog",
    "catalog-held",
    "prune",
    "bld-fetch",
    "osm-pass",
    "gc",
    "backup",
    "inputs",
    "names-todo",
    "timetables-todo",
    "regions-extract",
];

/// Whether the lead offers its members `step`'s targets: a step of the steps
/// table of targets by area (not a worldwide one: `names_target` of its name, a run alone, the
/// spoken languages) that isn't the lead's (`LEAD_ONLY`) and doesn't keep the pass's answers
/// (crate::agent::steps::ANSWERED), the shared steps among them.
pub fn offered(step: &str) -> bool {
    super::steps::row(step).is_some()
        && !LEAD_ONLY.contains(&step)
        && !super::steps::ANSWERED.contains(&step)
        && names_target(step)
        && !super::steps::alone(step)
        && step != "spoken"
}

/// The authority rule: whether work of `step` may be granted to the asker, `lead` when it's the
/// current lead's member (its own agent's ask, in process). The lead's own planned job (`own`) and a
/// step not `offered` to every member go to the lead alone; an offered step to any member.
pub fn grants(step: &str, own: bool, lead: bool) -> bool {
    lead || (!own && offered(step))
}

/// Whether a job's command names target `t` (a step's worldwide target, its own name or a world
/// root, isn't named: its step's job does the whole world), the lead's own jobs' and a member's
/// alike.
pub fn names_target(t: &str) -> bool {
    !matches!(
        t,
        "catalog"
            | "items"
            | "marks"
            | "roadunits"
            | "stations"
            | "ferries"
            | "heritage-sites"
            | "heritage"
            | "overlays"
            | "rail-feeds"
            | "rail"
            | "bld-fetch"
            | "terrain-water"
    ) && !t.ends_with("-root")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_step_but_the_leads_and_the_answers_is_offered() {
        for s in ["pack", "lo", "terrain-lo", "slope-lo", "trees-lo", "bldfield-lo"] {
            assert!(offered(s), "{s}");
        }
        for s in super::super::steps::SHARED {
            assert!(offered(s), "{s}");
        }
        for s in [
            "catalog",
            "catalog-held",
            "prune",
            "bld-fetch",
            "osm-pass",
            "gc",
            "inputs",
            "items",
            "heritage",
            "heritage-sites",
            "tail",
            "roadunits",
            "stations",
            "ferries",
            "terrain-root",
            "slope-root",
            "marks",
            "overlays",
            "rail",
            "rail-feeds",
            "terrain-water",
            "water",
            "labels",
            "spoken",
            "a step of a newer app",
        ] {
            assert!(!offered(s), "{s}");
        }
    }

    #[test]
    fn the_leads_steps_and_own_jobs_are_granted_to_the_lead_alone() {
        for s in ["catalog", "bld-fetch", "osm-pass", "gc", "items", "heritage", "roadunits", "terrain-root"] {
            assert!(grants(s, true, true) && grants(s, false, true), "{s}");
            assert!(!grants(s, false, false) && !grants(s, true, false), "{s}");
        }
        for s in ["pack", "lo", "slope", "unit"] {
            assert!(grants(s, false, true) && grants(s, false, false), "{s}");
            assert!(!grants(s, true, false), "{s}");
        }
    }

    #[test]
    fn worldwide_targets_are_not_named() {
        assert!(names_target("6/1/2") && names_target("3/2/2"));
        assert!(!names_target("roadunits") && !names_target("terrain-root") && !names_target("slope-root"));
    }
}
