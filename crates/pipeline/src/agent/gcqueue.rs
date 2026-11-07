//! GC's queue on the lead (docs/maplog.md §7.2, §7.4; §8 step 7: in shadow), the queue itself
//! store::map::gc. The lead queues each content name as it stops being held: a batch of the map's
//! log frees it (`Batch::freed`, told by the log's writer, super::publish), or a save of the
//! records drops it (the manifest's names and the pending uploads': `records`), while the other
//! holder doesn't name it. A name named again leaves the queue. A queued name is **due** when every
//! client's cursor heard within `CURSOR_TTL_S` (the NAS snapshot's among them: `snapshot`) is at or
//! past the `seq` it was freed at, `GRACE_S` has passed since, every live lease was granted after
//! it, the records have been seen and neither holder names it.
//!
//! The queue lives beside the map's log, in the lead's `maplog/` (store::map::gc::FILE), so it
//! survives a lead restart. A lead disk restore loses it: what it held is found again by the orphan
//! sweep (§7.5), step 8's, as are names dropped while no lead ran.
//!
//! What the queue has been told was deleted, with the newest lease granted then, is what the merge
//! refuses (`refusal`): a hand-off of a lease granted before the deletion that brings a deleted
//! name may have found the file on the NAS and reused it; its targets are done again. The `gc` job
//! tells the lead each name before it removes it (crate::agent::gc::Tell, `/gc/removing`: step 8a,
//! docs/maplog.md §11.4), so the refusal covers every removal from the moment it can happen; a name
//! something holds as it's told (GcQueue::removing: the map or the records name it, or it hasn't
//! outlasted a cursor, a lease or its grace) is answered and stays, the job's roots being as old as
//! its run.
//!
//! Shadow (step 7): nothing deletes by the queue. The `gc` job, which deletes by its own rules, is
//! given the queue's due set (`SHADOW_DUE`, written as its job is planned) and checks it against
//! what it removes that day (crate::agent::gc::Report::shadow): the shadow's due set should be a
//! part of today's deletions. Its check (`SHADOW_CHECK`) is read back at the next plan: the due
//! names it found gone already are marked deleted there (those it removed were, as it went).

use crate::pool::journal::{Entry, LeaseId};
use crate::pool::records::Records;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub use store::map::gc::{GcQueue, Holders, Queued, CURSOR_TTL_S, FILE, GRACE_S};

/// The due set handed to the `gc` job (`--shadow-due`), in the map log's folder.
pub const SHADOW_DUE: &str = "gc-shadow.json";
/// The `gc` job's check of it, beside it.
pub const SHADOW_CHECK: &str = "gc-shadow-check.json";

/// The due set handed to the `gc` job, and its check; with what the lead's map holds, GC's roots
/// besides the records (no catalogue names them since docs/maplog.md §8, step 6).
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShadowDue {
    /// When it was found (unix s).
    pub at: u64,
    /// The names queued then, and of them the due.
    pub queued: usize,
    pub names: Vec<String>,
    /// The content names the lead's map names then (GcQueue::map_names).
    #[serde(default)]
    pub map: Vec<String>,
    /// The names queued then, each with when it was freed (unix s): those freed within GC's window
    /// are kept as the window's maps would have kept them.
    #[serde(default)]
    pub freed: Vec<(String, u64)>,
}

/// The `gc` job's check of the shadow's due set against what it removed (crate::agent::gc).
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShadowCheck {
    /// The due set's size and when it was found.
    pub due: usize,
    pub due_at: u64,
    /// Due and removed by today's GC (a dry run: would be).
    pub removed: Vec<String>,
    /// Due and not on the NAS (gone before).
    pub gone: Vec<String>,
    /// Due and kept by today's GC, by its reason. A holder the queue missed (`MISSED`): `records`
    /// (the manifest, the records or a hand-off not merged name it), `map` (the map now names it).
    /// Expected while today's GC keeps its 14-day window: `window` (freed within it, or an older
    /// map snapshot on the NAS names it), `young` (younger than its window, or touched as it was to
    /// go), `not swept` (in no folder it sweeps: the queue covers more than its folders).
    pub kept: BTreeMap<String, Vec<String>>,
}

/// The reasons that mean the queue missed a holder: a due name it would have deleted while held.
pub const MISSED: [&str; 2] = ["records", "map"];

impl ShadowCheck {
    /// Whether no due name is held by what the queue should have seen (`MISSED`). Names today's GC
    /// keeps for its window alone don't count: expected every day until step 8 ends the window.
    pub fn holds(&self) -> bool {
        MISSED.iter().all(|r| self.kept.get(*r).is_none_or(Vec::is_empty))
    }

    /// In words, for the log and GC's note.
    pub fn said(&self) -> String {
        let count = |missed: bool| -> Vec<String> {
            self.kept
                .iter()
                .filter(|(k, v)| !v.is_empty() && MISSED.contains(&k.as_str()) == missed)
                .map(|(k, v)| format!("{} {k}", v.len()))
                .collect()
        };
        let (missed, window) = (count(true), count(false));
        let mut s = format!(
            "GC's queue (shadow): {} due, {} removed by today's GC, {} gone already",
            self.due,
            self.removed.len(),
            self.gone.len(),
        );
        if !window.is_empty() {
            s += &format!(", kept for its window: {}", window.join(", "));
        }
        if !missed.is_empty() {
            let first: Vec<&str> =
                MISSED.iter().filter_map(|r| self.kept.get(*r)).flatten().take(3).map(String::as_str).collect();
            s += &format!(", HELD (a holder the queue missed): {} (e.g. {})", missed.join(", "), first.join(", "));
        }
        s
    }
}

/// A lease's id as the queue keeps it.
pub fn lease(l: LeaseId) -> store::map::gc::Lease {
    store::map::gc::Lease { term: l.term, n: l.n }
}

/// The queue told of records `r` as saved (at `now`).
pub fn records(q: &GcQueue, r: &Records, now: u64) {
    q.records((r.term, r.seq), r.manifest.values().chain(r.pending.keys()), now);
}

/// Why the merge refuses journal entry `e`: a content name it brings was deleted after its lease
/// was granted. None: none was.
pub fn refusal(q: &GcQueue, e: &Entry) -> Option<String> {
    let h = &e.handoff;
    let (n, at) = q.deleted_under(h.changes.values().flatten().chain(h.pending.keys()), lease(e.lease))?;
    Some(format!(
        "{n} was removed from the NAS by GC (told at {at}, before it went) after its lease {} was granted (its job may have found it there): its targets are done again",
        e.lease
    ))
}

#[cfg(test)]
mod tests;
