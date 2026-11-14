//! Phases renamed whose old names kept data still holds, by step: a record read takes the current
//! name of an old one (crate::timings' `RunRec`, its phases at every level and its gaps;
//! agentcore::pubtimes' catalogue stats in the history), so a step's history keeps its phases across
//! a rename. Keyed by step: two steps may say the same words of different work (slope-lo's
//! `uploading the lo pack` is its own still; terrain-lo's became `writing the lo pack`).
//!
//! A row stays while some data holds its old name, and goes once none does; its new name is one a
//! step emits and the build page describes (tests/phase_names.rs), or, in `ENDED`, the last name of
//! a phase no step runs now. What holds them:
//! - catalog's, from before task #844 (30 Oct 2026) and #883 (1 Nov: the last catalogues no longer
//!   listed): the history's catalog events to 1 Nov, which the NAS's history archive keeps for good
//!   (agent::histarchive; agentcore::pubtimes reads any day of it);
//! - terrain-deep's and terrain-lo's, task #1097 (11 Nov 2026): the Macs' own timings logs
//!   (`<agent>/timings.jsonl`, `scenic timings --here`; the lead's `coord/timings.jsonl` too, should
//!   one hold them), until no Mac's holds a terrain-deep or terrain-lo run with the old name (a log
//!   past crate::timings::LOG_MAX keeps its newer half).

/// The current name of `step`'s phase once named `name` (itself when it wasn't renamed).
pub fn current<'a>(step: &str, name: &'a str) -> &'a str {
    [RENAMED, ENDED]
        .into_iter()
        .find_map(|rows| rows.binary_search_by(|(s, old, _)| (*s, *old).cmp(&(step, name))).ok().map(|i| rows[i].2))
        .unwrap_or(name)
}

/// Step, old name, current name; sorted by step and old name.
pub const RENAMED: &[(&str, &str, &str)] = &[
    ("catalog", "listing the last catalogs and reading the records' sizes", "reading the records' sizes"),
    ("catalog", "records read", "reading records"),
    ("catalog", "the catalog written", "writing the catalog"),
    ("catalog", "the coverage drawn and the credits found", "drawing the coverage and finding the credits"),
    ("catalog", "the last catalogs listed and the records' sizes read", "reading the records' sizes"),
    ("catalog", "the manifest's files sorted into the catalog's", "sorting the manifest's files into the catalog's"),
    ("catalog", "the new files' sizes", "reading the new files' sizes"),
    ("catalog", "the units' summaries read", "reading the units' summaries"),
    ("terrain-deep", "writing and uploading its deep pack and mid", "writing its deep pack and mid"),
    ("terrain-lo", "uploading the lo pack", "writing the lo pack"),
];

/// Step, old name, last name of a phase no step runs now (the catalogue's whole check, gone with
/// the catalogues in task #883), as `RENAMED`: its history's two names one phase.
pub const ENDED: &[(&str, &str, &str)] =
    &[("catalog", "every file checked on the NAS", "checking every file on the NAS")];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_names_read_as_new_by_step() {
        for rows in [RENAMED, ENDED] {
            assert!(rows.windows(2).all(|w| (w[0].0, w[0].1) < (w[1].0, w[1].1)), "sorted, no repeats");
        }
        assert_eq!(current("terrain-lo", "uploading the lo pack"), "writing the lo pack");
        // (Another step's phase of the same words keeps them.)
        assert_eq!(current("slope-lo", "uploading the lo pack"), "uploading the lo pack");
        assert_eq!(current("catalog", "records read"), "reading records");
        assert_eq!(current("catalog", "reading records"), "reading records");
        assert_eq!(current("catalog", "every file checked on the NAS"), "checking every file on the NAS");
        // (No new name is an old one of its step: a record read twice keeps its names.)
        assert!(RENAMED.iter().chain(ENDED).all(|(s, _, new)| current(s, new) == *new));
    }
}
