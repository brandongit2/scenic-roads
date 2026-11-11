//! GC's queue on the lead (docs/maplog.md §7.2 to §7.5, §8 step 8b), the queue itself
//! store::map::gc. The lead queues each content name as it stops being held: a batch of the map's
//! log frees it (`Batch::freed`, told by the log's writer, super::publish), or a save of the
//! records drops it (the manifest's names and the pending uploads': `records`), while the other
//! holder doesn't name it; the orphan sweep queues what it finds on the NAS that nothing holds
//! (crate::agent::gc, §7.5). A name named again leaves the queue. A queued name is **due** when
//! every client's cursor heard within `CURSOR_TTL_S` (the NAS snapshot's among them) is at or past
//! the `seq` it was freed at, `GRACE_S` has passed since, every live lease was granted after it,
//! the records have been seen and no holder names it.
//!
//! The queue lives beside the map's log, in the lead's `maplog/` (store::map::gc::FILE), so it
//! survives a lead restart. A lead disk lost loses it: what it held is found again by the orphan
//! sweep, as are names dropped while no lead ran.
//!
//! The `gc` job takes the due names in chunks through the lead's coordinator (crate::agent::gc,
//! `/gc/chunk`): each chunk marked deleted, the newest lease granted then theirs, before any of it
//! goes; then removed, and said gone (`/gc/gone`). A chunk told and not said gone (the job killed)
//! is the next chunk's first, told again. What's marked, being removed or gone, is what the merge
//! refuses (`refusal`): a hand-off bringing a name being removed, or one deleted after its lease
//! was granted (its job may have found the file on the NAS before it went, and reused it); its
//! targets are done again. A hand-off the merge takes holds the names it brings (its changes, its
//! uploads and the files it reused: buildstore::handoff::Handoff::reused) from every chunk until the
//! records after it are seen.

use crate::pool::journal::{Entry, LeaseId};
use crate::pool::records::Records;
pub use store::map::gc::{GcQueue, Holders, Queued, Refused, CURSOR_TTL_S, FILE, GRACE_S};

/// A lease's id as the queue keeps it.
pub fn lease(l: LeaseId) -> store::map::gc::Lease {
    store::map::gc::Lease { epoch: l.epoch, n: l.n }
}

/// The queue told of records `r` as saved (at `now`).
pub fn records(q: &GcQueue, r: &Records, now: u64) {
    q.records((r.epoch, r.seq), r.manifest.values().chain(r.pending.keys()), now);
}

/// Why the merge refuses journal entry `e` (at `now`): a content name it brings (a change, an
/// upload, a file it reused) is being removed, or was deleted after its lease was granted. None:
/// it may merge, and its names are held until the records after it are seen.
pub fn refusal(q: &GcQueue, e: &Entry, now: u64) -> Option<String> {
    let h = &e.handoff;
    let names = h.changes.values().flatten().chain(h.pending.keys()).chain(h.reused.iter());
    Some(match q.bringing(names, lease(e.lease), now)? {
        Refused::Deleted(n, at) => format!(
            "{n} was removed from the NAS by GC (told at {at}, before it went) after its lease {} was granted (its job may have found it there): its targets are done again",
            e.lease
        ),
        Refused::Removing(n) => format!(
            "{n} is being removed from the NAS by GC (told, not yet gone), and lease {}'s job found it there: its targets are done again",
            e.lease
        ),
    })
}

#[cfg(test)]
mod tests;
