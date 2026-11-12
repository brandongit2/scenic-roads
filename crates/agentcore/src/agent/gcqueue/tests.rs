use super::*;
use crate::pool::records::{self, Records};
use buildstore::handoff::Handoff;
use serde_json::json;
use std::collections::BTreeMap;
use store::map::{Batch, Cursor, Map};

const T0: u64 = 1_800_000_000;

/// A map at `seq` naming each of `files` (logical `<l>` → content `<l>.<k>.pack`).
fn map(seq: u64, files: &[(&str, &str)]) -> Map {
    let e = files.iter().map(|(l, c)| (format!("f/{l}"), json!({ "file": c, "size": 1, "fmt": 1 }).into())).collect();
    Map::new(seq, format!("t{seq}"), e)
}

/// The batch from `m` to the map naming `files` at `seq`.
fn to(m: &Map, files: &[(&str, &str)], seq: u64) -> (Batch, Map) {
    let next = map(seq, files);
    let b = m.batch_to(next.entries(), seq, &format!("t{seq}"), "", "");
    let after = m.apply(&b).unwrap();
    (b, after)
}

fn records(seq: u64, manifest: &[(&str, &str)]) -> Records {
    Records {
        epoch: 2,
        seq,
        manifest: manifest.iter().map(|(l, c)| (l.to_string(), c.to_string())).collect(),
        ..Default::default()
    }
}

/// A name a batch frees is queued (the records don't name it), and leaves the queue when a later
/// batch names it again, or a records save does; what's queued survives a lead restart, and a name
/// the records hold is never queued.
#[test]
fn a_name_freed_and_taken_again_leaves_the_queue() {
    let d = tempfile::tempdir().unwrap();
    let m1 = map(1, &[("a", "a.1.pack"), ("b", "b.1.pack"), ("c", "c.1.pack")]);
    let q = GcQueue::default();
    q.open(d.path(), &m1).unwrap();
    super::records(&q, &records(1, &[("c", "c.1.pack")]), T0);
    // (Batch 2 renames a, b and c: their old names freed; c's still the records'.)
    let (b2, m2) = to(&m1, &[("a", "a.2.pack"), ("b", "b.2.pack"), ("c", "c.2.pack")], 2);
    q.batch(&b2, T0 + 10);
    assert_eq!(q.queued("a.1.pack"), Some(Queued { seq: 2, at: T0 + 10 }));
    assert!(q.queued("b.1.pack").is_some());
    assert!(q.queued("c.1.pack").is_none(), "the records hold it");
    // (Batch 3 names a.1 again: taken.)
    let (b3, _) = to(&m2, &[("a", "a.1.pack"), ("b", "b.2.pack"), ("c", "c.2.pack")], 3);
    q.batch(&b3, T0 + 20);
    assert!(q.queued("a.1.pack").is_none());
    // (A records save naming b.1: taken; one dropping c.1: queued now, at the map's seq.)
    super::records(&q, &records(2, &[("b", "b.1.pack")]), T0 + 30);
    assert!(q.queued("b.1.pack").is_none());
    assert_eq!(q.queued("c.1.pack"), Some(Queued { seq: 3, at: T0 + 30 }));
    // (a.2 freed by batch 3: queued.)
    assert!(q.queued("a.2.pack").is_some());
    // A lead restart: the same queue from its file.
    let back = GcQueue::default();
    back.open(d.path(), &q_map(&m2, &b3)).unwrap();
    for n in ["a.1.pack", "b.1.pack", "c.1.pack", "a.2.pack"] {
        assert_eq!(back.queued(n), q.queued(n), "{n}");
    }
    assert_eq!(back.len(), q.len());
}

fn q_map(m: &Map, b: &Batch) -> Map {
    m.apply(b).unwrap()
}

/// A queued name is due only once every cursor heard within the TTL is at or past its `seq`, the
/// grace has passed, every live lease was granted after it was freed, the records have been seen
/// and no holder names it.
#[test]
fn due_waits_for_every_cursor_the_grace_and_older_leases() {
    let d = tempfile::tempdir().unwrap();
    let m1 = map(1, &[("a", "a.1.pack")]);
    let q = GcQueue::default();
    q.open(d.path(), &m1).unwrap();
    let (b2, _) = to(&m1, &[("a", "a.2.pack")], 2);
    q.batch(&b2, T0);
    let cur = |seq: u64, heard: u64| Cursor { seq, heard, at: heard, ..Default::default() };
    let late = T0 + GRACE_S;
    let all_past: BTreeMap<String, Cursor> =
        [("server/m4".to_string(), cur(2, late)), ("snapshot".to_string(), cur(2, late))].into();
    let due = |c: &BTreeMap<String, Cursor>, lease: Option<u64>, now: u64| {
        q.due(&Holders { cursors: c, oldest_lease: lease, now })
    };
    // (No records seen yet: a holder unknown, nothing due.)
    assert!(due(&all_past, None, late).is_empty());
    super::records(&q, &records(1, &[]), T0);
    assert_eq!(due(&all_past, None, late), vec!["a.1.pack".to_string()]);
    // A cursor still before the batch that freed it holds it…
    let mut behind = all_past.clone();
    behind.insert("server/m1".into(), cur(1, late));
    assert!(due(&behind, None, late).is_empty());
    // …until it's unheard for the TTL.
    behind.insert("server/m1".into(), cur(1, late - CURSOR_TTL_S));
    assert_eq!(due(&behind, None, late).len(), 1);
    // The grace.
    assert!(due(&all_past, None, late - 1).is_empty());
    // A live lease granted before it was freed (or at once) holds it; one granted after doesn't.
    assert!(due(&all_past, Some(T0 - 5), late).is_empty());
    assert!(due(&all_past, Some(T0), late).is_empty());
    assert_eq!(due(&all_past, Some(T0 + 1), late).len(), 1);
    // A holder naming it again: not due (a records save takes it off the queue too).
    super::records(&q, &records(2, &[("a", "a.1.pack")]), late);
    assert!(due(&all_past, None, late).is_empty());
}

/// Task #1134: a laptop's map server, on demand, asks while it runs and then quits, its cursor
/// left behind at the map it was at; the lead's queue held 54,575 names (all but 1,029) behind
/// the M1's, unheard 4 days, for its 7-day TTL, while the mini's server and mirror and the NAS
/// snapshot were long past them. Unheard `CURSOR_TTL_S` (hours), it holds nothing; asking again,
/// it holds again from its `seq`. The NAS snapshot's cursor, which the lead notes as it writes one
/// (on a publish, so hours apart when the build publishes nothing), holds however long ago.
#[test]
fn a_quit_servers_cursor_holds_gc_hours_not_days_and_the_snapshots_always() {
    let d = tempfile::tempdir().unwrap();
    let m1 = map(1, &[("a", "a.1.pack")]);
    let q = GcQueue::default();
    q.open(d.path(), &m1).unwrap();
    super::records(&q, &records(1, &[]), T0);
    let (b2, m2) = to(&m1, &[("a", "a.2.pack")], 2);
    let (b3, m3) = to(&m2, &[("a", "a.3.pack")], 3);
    q.batch(&b2, T0);
    q.batch(&b3, T0);
    let feed = store::map::feed::Feed::new();
    feed.start(std::sync::Arc::new(m3), 1, &[b2, b3], BTreeMap::new());
    // The M1's server at map 1, then gone; the mini's long-polling at the head; the snapshot at 3.
    feed.changes(1, Some("server/m1"), T0);
    feed.note_snapshot(3, T0);
    let due_at = |now: u64| {
        feed.changes(3, Some("server/mini"), now);
        q.due(&Holders { cursors: &feed.cursors(), oldest_lease: None, now })
    };
    let both = vec!["a.1.pack".to_string(), "a.2.pack".to_string()];
    assert!(due_at(T0 + GRACE_S).is_empty(), "the M1's cursor at 1 holds both");
    assert!(due_at(T0 + CURSOR_TTL_S - 1).is_empty(), "heard within the TTL");
    assert_eq!(due_at(T0 + CURSOR_TTL_S), both, "unheard the TTL: nothing held, the snapshot's at 3 neither");
    assert!(CURSOR_TTL_S <= 6 * 3600, "hours, not days: each hour keeps an hour's freed files");
    // Back, at 1 still: it holds them again until it moves on.
    feed.changes(1, Some("server/m1"), T0 + CURSOR_TTL_S + 5);
    assert!(due_at(T0 + CURSOR_TTL_S + 10).is_empty());
    // The snapshot's cursor, noted at 2 long ago, holds what map 2 names: a.2, never a.1.
    feed.changes(3, Some("server/m1"), T0 + CURSOR_TTL_S + 20);
    feed.note_snapshot(2, T0);
    assert_eq!(due_at(T0 + 9 * CURSOR_TTL_S), vec!["a.1.pack".to_string()], "the snapshot's never lapses");
}

fn entry(n: u64, content: &str) -> Entry {
    let h = Handoff {
        changes: [("base/6-1-1".to_string(), Some(content.to_string()))].into(),
        pending: [(content.to_string(), "sha".to_string())].into(),
        done: Some(("unit".into(), vec![("6-1-1".into(), format!("k{n}"))])),
        ..Default::default()
    };
    Entry {
        member: "m-000000000000000b".into(),
        job: JobId { epoch: 2, n },
        step: "unit".into(),
        handoff: h,
        at: T0 + n,
    }
}

/// A hand-off of a lease granted before a name it brings was deleted is refused by the merge (its
/// job may have found the file on the NAS before it went): its target stays unrecorded, so the plan
/// offers it again; the redo's hand-off, a lease granted after the deletion, merges.
#[test]
fn a_reuse_of_a_deleted_name_is_refused_and_redone() {
    let d = tempfile::tempdir().unwrap();
    let q = GcQueue::default();
    q.open(d.path(), &map(1, &[])).unwrap();
    let name = "base/6-1-1.k0.base";
    super::records(&q, &records(1, &[]), T0);
    // (Told and removed while leases up to 2-7 were granted.)
    let none = BTreeMap::new();
    let h = Holders { cursors: &none, oldest_lease: None, now: T0 + 100 };
    let at = lease(JobId { epoch: 2, n: 7 });
    assert!(q.removing(&[name.to_string()], &h, at).unwrap().is_empty());
    q.gone(&[name.to_string()], &[], T0 + 100, at).unwrap();
    let mut r = Records { epoch: 2, ..Default::default() };
    let check = |e: &Entry, _: &Records| refusal(&q, e, T0 + 150).map_or(Ok(()), Err);
    let m = records::merge(&mut r, [&entry(5, name)], &check);
    assert_eq!(m.refused.len(), 1, "{m:?}");
    assert!(m.refused[0].1.contains(name), "{}", m.refused[0].1);
    assert_eq!(r.keys.recorded("unit", "6-1-1"), None, "offered again");
    assert!(!r.manifest.contains_key("base/6-1-1"));
    // The redo, its lease granted after the deletion (its file uploaded afresh): merged.
    let m = records::merge(&mut r, [&entry(8, name)], &check);
    assert_eq!(m.applied.len(), 1, "{m:?}");
    assert_eq!(r.manifest["base/6-1-1"], name);
    // A name not deleted: no refusal. And the deletion survives a restart.
    assert!(refusal(&q, &entry(3, "base/6-1-1.k1.base"), T0 + 150).is_none());
    let back = GcQueue::default();
    back.open(d.path(), &map(1, &[])).unwrap();
    assert!(refusal(&back, &entry(5, name), T0 + 200).is_some());
    assert!(refusal(&back, &entry(8, name), T0 + 200).is_none());
}

/// docs/maplog.md §7.4 (step 8b, no touch): a hand-off bringing a name a chunk is removing is
/// refused whatever its lease (its job found the file there before it went); the mark's lease is
/// the newest when the removal ended, so a lease granted while it went is refused after it too. A
/// hand-off the merge takes holds its names (its changes, uploads and reused files) from the
/// chunks until the records after it are seen; then what they don't name is queued again from
/// when it was brought.
#[test]
fn a_reuse_of_a_name_being_removed_is_refused_and_a_merge_holds_its_names() {
    let d = tempfile::tempdir().unwrap();
    let m1 = map(1, &[("a", "a.1.pack"), ("b", "b.1.pack"), ("c", "c.1.pack")]);
    let q = GcQueue::default();
    q.open(d.path(), &m1).unwrap();
    super::records(&q, &records(1, &[]), T0);
    let (b2, _) = to(&m1, &[], 2);
    q.batch(&b2, T0);
    let none = BTreeMap::new();
    let past = T0 + GRACE_S;
    let h = Holders { cursors: &none, oldest_lease: None, now: past };
    let told = store::map::gc::Lease { epoch: 2, n: 5 };
    // (A hand-off brings c, reused: held from the chunk, which takes a and b.)
    let reuse = |n: u64, names: &[&str]| Entry {
        handoff: Handoff { reused: names.iter().map(|s| s.to_string()).collect(), ..Default::default() },
        ..entry(n, "x.9.pack")
    };
    assert!(refusal(&q, &reuse(9, &["c.1.pack"]), past).is_none());
    assert_eq!(q.chunk(&h, told, 10, &|_| true).unwrap(), ["a.1.pack", "b.1.pack"]);
    // Being removed: refused, however new its lease.
    let newer = Entry { job: JobId { epoch: 3, n: 1 }, ..reuse(1, &["a.1.pack"]) };
    assert!(refusal(&q, &newer, past).is_some_and(|w| w.contains("being removed")), "while it goes");
    // Gone, its mark's lease the newest then (3-4): a lease up to it refused, one after not.
    q.gone(&["a.1.pack".into()], &["b.1.pack".into()], past, store::map::gc::Lease { epoch: 3, n: 4 }).unwrap();
    assert!(refusal(&q, &newer, past).is_some());
    assert!(refusal(&q, &Entry { job: JobId { epoch: 3, n: 5 }, ..newer.clone() }, past).is_none());
    // Let go (yielded on): b queued again as it was, unmarked; a hand-off bringing it merges.
    assert_eq!(q.queued("b.1.pack"), Some(Queued { seq: 2, at: T0 }));
    assert!(refusal(&q, &reuse(2, &["b.1.pack"]), past).is_none());
    // The records after them, naming neither b nor c: both queued again from when brought.
    super::records(&q, &records(2, &[]), past + 10);
    assert_eq!(q.queued("c.1.pack").map(|x| x.at), Some(past));
    assert_eq!(q.queued("b.1.pack").map(|x| x.at), Some(past));
    assert!(q.due(&h).is_empty(), "their grace from then");
}

/// docs/maplog.md §8 step 8b's deploy: a hand-off from a member on an app before it says no
/// `reused` (its app touched the file instead); every file its job put under a logical name, reused
/// or not, is among its changes, which the merge checks as it does `reused`, so one bringing a name
/// being removed, or removed under its lease, is refused all the same. (The gate's copies, the one
/// reuse no change names, are the lead's `inputs` step's: on the lead's app.)
#[test]
fn an_older_member_s_hand_off_without_reused_is_refused_by_its_changes() {
    let d = tempfile::tempdir().unwrap();
    let q = GcQueue::default();
    q.open(d.path(), &map(1, &[])).unwrap();
    super::records(&q, &records(1, &[]), T0);
    let name = "base/6-1-1.k0.base";
    let old: Handoff = serde_json::from_value(json!({
        "changes": { "base/6-1-1": name },
        "pending": {},
        "checked": [],
        "sizes": {},
        "done": ["unit", [["6-1-1", "k5"]]],
    }))
    .unwrap();
    assert!(old.reused.is_empty());
    let e = Entry { handoff: old, ..entry(5, name) };
    let none = BTreeMap::new();
    let h = Holders { cursors: &none, oldest_lease: None, now: T0 + 100 };
    assert!(q.removing(&[name.to_string()], &h, lease(JobId { epoch: 2, n: 7 })).unwrap().is_empty());
    assert!(refusal(&q, &e, T0 + 100).is_some_and(|w| w.contains("being removed")));
    q.gone(&[name.to_string()], &[], T0 + 100, lease(JobId { epoch: 2, n: 7 })).unwrap();
    assert!(refusal(&q, &e, T0 + 100).is_some_and(|w| w.contains("removed from the NAS")));
}

/// A chunk told and not said gone (the job killed) is the next chunk's first, told again against
/// the holders as of then: one the records now name kept (off the chunk and the queue, but still
/// marked: the killed job may have removed it, so a hand-off of a lease granted before it was told
/// is still refused), the rest marked again with the newest lease; the queue's file keeps it across
/// a lead restart.
#[test]
fn a_chunk_told_again_keeps_what_is_held_now() {
    let d = tempfile::tempdir().unwrap();
    let m1 = map(1, &[("a", "a.1.pack"), ("b", "b.1.pack"), ("c", "c.1.pack"), ("e", "e.1.pack")]);
    let q = GcQueue::default();
    q.open(d.path(), &m1).unwrap();
    super::records(&q, &records(1, &[]), T0);
    let (b2, m2) = to(&m1, &[], 2);
    q.batch(&b2, T0);
    let none = BTreeMap::new();
    let h = Holders { cursors: &none, oldest_lease: None, now: T0 + GRACE_S };
    let (l1, l2) = (store::map::gc::Lease { epoch: 2, n: 1 }, store::map::gc::Lease { epoch: 2, n: 8 });
    assert_eq!(q.chunk(&h, l1, 3, &|_| true).unwrap(), ["a.1.pack", "b.1.pack", "c.1.pack"]);
    // (Killed; the lead restarts, its records naming b now.)
    let back = GcQueue::default();
    back.open(d.path(), &m2).unwrap();
    assert_eq!(back.removing_now(), ["a.1.pack", "b.1.pack", "c.1.pack"]);
    super::records(&back, &records(2, &[("b", "b.1.pack")]), T0 + GRACE_S);
    assert_eq!(back.chunk(&h, l2, 3, &|_| true).unwrap(), ["a.1.pack", "c.1.pack", "e.1.pack"]);
    assert!(back.queued("b.1.pack").is_none() && !back.removing_now().contains(&"b.1.pack".to_string()));
    let b = "b.1.pack".to_string();
    assert!(back.deleted_under([&b].into_iter(), l1).is_some(), "still marked: it may be gone");
    let reuse = Entry { handoff: Handoff { reused: vec![b.clone()], ..Default::default() }, ..entry(1, "x.9.pack") };
    assert!(refusal(&back, &reuse, T0 + GRACE_S).is_some(), "a hand-off of an older lease bringing it");
    // (And across a restart.)
    let again = GcQueue::default();
    again.open(d.path(), &m2).unwrap();
    assert!(again.deleted_under([&b].into_iter(), l1).is_some() && !again.removing_now().contains(&b));
    assert!(
        back.deleted_under([&"a.1.pack".to_string()].into_iter(), store::map::gc::Lease { epoch: 2, n: 8 }).is_some(),
        "marked again, newer"
    );
    // Yielding at once on it: e (told the first time) let go unmarked; a and c, which the killed
    // job may have removed, kept marked (still queued, taken again by the next chunk), across a
    // restart too.
    let ace: Vec<String> = ["a.1.pack", "c.1.pack", "e.1.pack"].map(String::from).to_vec();
    back.gone(&[], &ace, T0 + GRACE_S, l2).unwrap();
    let reopened = GcQueue::default();
    reopened.open(d.path(), &m2).unwrap();
    for q in [&back, &reopened] {
        let marked = |n: &str| q.deleted_under([&n.to_string()].into_iter(), l1).is_some();
        assert!(marked("a.1.pack") && marked("c.1.pack") && !marked("e.1.pack"));
        assert!(q.removing_now().is_empty() && ace.iter().all(|n| q.queued(n).is_some()));
    }
    assert_eq!(back.chunk(&h, l2, 3, &|_| true).unwrap(), ace);
    back.gone(&ace, &[], T0 + GRACE_S, l2).unwrap();
    assert!(back.is_empty() && back.removing_now().is_empty());
    assert!(back.chunk(&h, l2, 3, &|_| true).unwrap().is_empty());
}

/// docs/maplog.md §11.4 (step 8a): the `gc` job tells the lead's coordinator each name before it
/// removes it (`/gc/removing`, crate::agent::gc::TheLead): marked deleted in the lead's queue, the
/// newest lease granted then theirs, at once and kept on its disk, so a hand-off (posted by HTTP or
/// in the journal, merged whenever) of a lease granted before brings it refused, and its redo
/// merges. A coordinator with no queue (not the lead, its log not open) marks nothing, and the job
/// is told so: it removes nothing.
#[test]
fn a_hand_off_reusing_a_name_gc_removes_is_refused() {
    use crate::agent::gc::{Lead, TheLead};
    let d = tempfile::tempdir().unwrap();
    let name = "base/6-1-1.k0.base";
    let (c, port) = crate::coord::start_for_test(&d.path().join("lead"), "m4", "");
    let lead = || {
        TheLead::at(crate::coord::client::Client::at(
            vec![format!("http://127.0.0.1:{port}")],
            c.contact.token.clone(),
            "m4",
        ))
    };
    let e = format!("{:#}", lead().removing(&[name.to_string()]).unwrap_err());
    assert!(e.contains("503") || e.contains("no GC queue"), "{e}");
    std::fs::create_dir_all(d.path().join("maplog")).unwrap();
    let q = std::sync::Arc::new(GcQueue::default());
    q.open(&d.path().join("maplog"), &map(1, &[])).unwrap();
    c.set_epoch(3);
    c.set_maplog(store::map::feed::Feed::new(), q.clone());
    // (The records not seen yet, a holder unknown: nothing marked, the job told so.)
    assert!(lead().removing(&[name.to_string()]).is_err());
    assert!(q.deleted_under([name.to_string()].iter(), lease(JobId { epoch: 0, n: 0 })).is_none());
    super::records(&q, &records(1, &[]), T0);
    assert_eq!(lead().removing(&[name.to_string()]).unwrap(), Vec::<String>::new());
    // (Being removed: refused, whatever its lease; said gone, a lease of term 2, granted before:
    // refused; the redo's, granted after: merged.)
    let reused = entry(5, name);
    assert!(refusal(&q, &reused, T0).is_some_and(|why| why.contains(name)));
    lead().gone(&[name.to_string()], &[]).unwrap();
    assert!(refusal(&q, &reused, T0).is_some_and(|why| why.contains(name)));
    let redo = Entry { job: JobId { epoch: 4, n: 1 }, ..entry(8, name) };
    assert!(refusal(&q, &redo, T0).is_none());
    let back = GcQueue::default();
    back.open(&d.path().join("maplog"), &map(1, &[])).unwrap();
    assert!(refusal(&back, &reused, T0).is_some(), "kept on the lead's disk");
    // The chunks and the orphans by HTTP alike: the records seen, nothing due, nothing queued.
    use crate::agent::gc::ChunkAsk;
    let c = lead().chunk(&ChunkAsk { max: 200, passes_before: None, dry: false }).unwrap();
    assert!(c.names.is_empty());
    assert_eq!(lead().orphans(&["base/6-1-2.k0.base".to_string()]).unwrap(), 1);
    assert!(q.queued("base/6-1-2.k0.base").is_some());
}

/// docs/maplog.md §11.4: the lead answers the `gc` job's names against its holders as they are now,
/// the job's roots being as old as its run: what the map or the records name, and a queued name that
/// hasn't outlasted its grace, a client's cursor or a live lease, stay unmarked and are answered, to
/// keep; the rest are marked. Marks that can't be on the disk (a queue with no file) are an error.
#[test]
fn the_lead_holds_back_what_is_held_as_gc_tells_it() {
    let d = tempfile::tempdir().unwrap();
    let m1 = map(1, &[("a", "a.1.pack"), ("b", "b.1.pack"), ("c", "c.1.pack")]);
    let q = GcQueue::default();
    q.open(d.path(), &m1).unwrap();
    super::records(&q, &records(1, &[("r", "r.1.pack")]), T0);
    let (b2, _) = to(&m1, &[("a", "a.1.pack"), ("c", "c.1.pack"), ("n", "n.1.pack")], 2);
    q.batch(&b2, T0);
    assert!(q.queued("b.1.pack").is_some());
    let names: Vec<String> = ["a.1.pack", "r.1.pack", "b.1.pack", "x.1.pack"].map(String::from).to_vec();
    let newest = store::map::gc::Lease { epoch: 2, n: 9 };
    let at = |now: u64, cursors: &BTreeMap<String, Cursor>, oldest_lease| {
        q.removing(&names, &Holders { cursors, oldest_lease, now }, newest).unwrap()
    };
    let none = BTreeMap::new();
    // Within its grace; a cursor behind it; a lease older than it. The map's and the records' always.
    assert_eq!(at(T0 + 10, &none, None), ["a.1.pack", "r.1.pack", "b.1.pack"]);
    let past = T0 + GRACE_S;
    let behind: BTreeMap<String, Cursor> =
        [("server/m1".to_string(), Cursor { seq: 1, heard: past, at: past, ..Default::default() })].into();
    assert_eq!(at(past, &behind, None), ["a.1.pack", "r.1.pack", "b.1.pack"]);
    assert_eq!(at(past, &none, Some(T0 - 1)), ["a.1.pack", "r.1.pack", "b.1.pack"]);
    assert!(q.queued("b.1.pack").is_some(), "still queued, not marked");
    // All passed: it's marked (and being removed, queued until gone); only the map's and the
    // records' stay.
    assert_eq!(at(past, &none, Some(past)), ["a.1.pack", "r.1.pack"]);
    assert!(q.queued("b.1.pack").is_some() && q.removing_now().contains(&"b.1.pack".to_string()));
    let older = store::map::gc::Lease { epoch: 2, n: 5 };
    assert!(q.deleted_under([&"b.1.pack".to_string()].into_iter(), older).is_some());
    assert!(q.deleted_under([&"x.1.pack".to_string()].into_iter(), older).is_some(), "named by nothing: marked");
    assert!(q.deleted_under([&"a.1.pack".to_string()].into_iter(), older).is_none(), "kept: not marked");
    let unopened = GcQueue::default();
    super::records(&unopened, &records(1, &[]), T0);
    let free = ["y.1.pack".to_string()];
    assert!(unopened.removing(&free, &Holders { cursors: &none, oldest_lease: None, now: past }, newest).is_err());
}

/// A last line cut short is dropped (and cut off the file); a damaged earlier line is an error.
#[test]
fn a_torn_last_line_is_dropped() {
    let d = tempfile::tempdir().unwrap();
    let m1 = map(1, &[("a", "a.1.pack"), ("b", "b.1.pack")]);
    let q = GcQueue::default();
    q.open(d.path(), &m1).unwrap();
    super::records(&q, &records(1, &[]), T0);
    let (b2, m2) = to(&m1, &[("a", "a.2.pack"), ("b", "b.1.pack")], 2);
    q.batch(&b2, T0);
    let p = d.path().join(FILE);
    let whole = std::fs::read(&p).unwrap();
    let mut torn = whole.clone();
    torn.extend_from_slice(b"{\"q\":\"b.1.pa");
    std::fs::write(&p, &torn).unwrap();
    let back = GcQueue::default();
    back.open(d.path(), &m2).unwrap();
    assert!(back.queued("a.1.pack").is_some());
    assert_eq!(std::fs::read(&p).unwrap(), whole);
    let mut bad = whole.clone();
    bad[2] = b'#';
    bad.extend_from_slice(b"{\"taken\":\"x\"}\n");
    std::fs::write(&p, &bad).unwrap();
    assert!(GcQueue::default().open(d.path(), &m2).is_err());
}

/// docs/maplog.md §7.4: a member away 10 days (the M4 travelling) with a hand-off not yet told,
/// whose job reused a name GC then removed: the deletion is kept for the merge as long as a lease
/// older than it may still be handed off (owed: granted to that member, not left out of a beat of
/// its since), however long, across a lead restart; its hand-off is refused and the redo merges.
/// Once no lease that old is owed (its member back, its hand-off merged and no longer listed), the
/// deletion is let go.
#[test]
fn a_member_away_ten_days_reusing_a_removed_name_is_refused_and_redone() {
    use crate::coord::{beats, held, lease::Work};
    let d = tempfile::tempdir().unwrap();
    let (c, _port) = crate::coord::start_for_test(&d.path().join("lead"), "m1", "");
    let dir = d.path().join("maplog");
    std::fs::create_dir_all(&dir).unwrap();
    let q = std::sync::Arc::new(GcQueue::default());
    q.open(&dir, &map(1, &[])).unwrap();
    c.set_epoch(3);
    c.set_maplog(store::map::feed::Feed::new(), q.clone());
    super::records(&q, &records(1, &[]), T0);
    let beat = |m: &str, held: &[u64], handoffs: &[String]| held::Beat {
        member: m.into(),
        speaks: vec![held::SPEAKS.into(), beats::HANDOFF.into()],
        held: held.iter().copied().collect(),
        handoffs: handoffs.to_vec(),
        ..Default::default()
    };
    let grant = |worker: &str| {
        let mut s = c.shared.lock().unwrap();
        let now = s.now();
        let work = Work::Job { step: "unit".into(), targets: vec![("6-1-1".into(), "k5".into())] };
        s.leases.grant_at(worker, work, now, T0)
    };
    // m4's job holds a lease; it says so, and goes away. m2 stays, beating.
    let id = grant("m4");
    c.member_beat(&beat("m4", &[id], &[]));
    c.member_beat(&beat("m2", &[], &[]));
    // Its job reused `name`, which GC removes a day later (the mark's lease the newest then).
    let name = "base/6-1-1.k0.base";
    let none = BTreeMap::new();
    let mark = lease(c.lease_bounds().1);
    let h = Holders { cursors: &none, oldest_lease: None, now: T0 + 86400 };
    assert!(q.removing(&[name.to_string()], &h, mark).unwrap().is_empty());
    q.gone(&[name.to_string()], &[], T0 + 86400, mark).unwrap();
    // Away: its lease released by the lead (12 h unheard), m2 beating all the while.
    c.shared.lock().unwrap().leases.drop_where(|l| l.id == id);
    for _ in 0..3 {
        c.member_beat(&beat("m2", &[], &[]));
    }
    assert_eq!(q.deletions_kept(), 1, "kept: m4 may still hand off under its lease");
    // A lead restart keeps what m4 owes.
    let back = GcQueue::default();
    back.open(&dir, &map(1, &[])).unwrap();
    back.leases(["m2".to_string(), "m4".to_string()].into(), None);
    assert_eq!(back.deletions_kept(), 1);
    // 10 days on, m4 back: its hand-off (reusing `name`) refused, its targets done again.
    let ten = T0 + 10 * 86400;
    let reused = Entry {
        job: JobId { epoch: 3, n: id },
        handoff: Handoff { reused: vec![name.to_string()], ..Default::default() },
        ..entry(5, "base/6-1-1.k5.base")
    };
    let key = reused.key().unwrap();
    c.member_beat(&beat("m4", &[], std::slice::from_ref(&key)));
    let why = refusal(&q, &reused, ten).expect("refused");
    assert!(why.contains(name), "{why}");
    let redo = Entry { job: JobId { epoch: 3, n: grant("m2") }, ..reused.clone() };
    assert!(refusal(&q, &redo, ten).is_none(), "the redo merges");
    assert_eq!(q.deletions_kept(), 1, "m4 still lists its hand-off");
    // Its hand-off answered, m4 lists nothing: no lease as old as the deletion is owed, let go.
    c.member_beat(&beat("m4", &[], &[]));
    assert_eq!(q.deletions_kept(), 0);
}

/// Task #1106: the grace (an hour) isn't what keeps a hand-off safe. m4's job, its lease granted
/// first, reuses a file a publish then frees; its live lease holds the file past the grace. m4
/// sleeps with its hand-off unposted: its lease released, the file is due once the grace has
/// passed from its freeing, and a chunk removes it; m4 back hours later, its hand-off is refused
/// (the deletion kept while m4 owes the lease, forgotten by the lead or not), its targets done
/// again, and the redo merges.
#[test]
fn a_hand_off_held_past_the_grace_reusing_a_name_gc_freed_is_refused() {
    use crate::coord::{beats, held, lease::Work};
    let d = tempfile::tempdir().unwrap();
    let (c, _port) = crate::coord::start_for_test(&d.path().join("lead"), "m1", "");
    let dir = d.path().join("maplog");
    std::fs::create_dir_all(&dir).unwrap();
    let name = "base/6-1-1.k0.base";
    let m1 = map(1, &[("base/6-1-1", name)]);
    let q = std::sync::Arc::new(GcQueue::default());
    q.open(&dir, &m1).unwrap();
    c.set_epoch(3);
    c.set_maplog(store::map::feed::Feed::new(), q.clone());
    super::records(&q, &records(1, &[]), T0);
    let beat = |m: &str, held: &[u64], handoffs: &[String]| held::Beat {
        member: m.into(),
        speaks: vec![held::SPEAKS.into(), beats::HANDOFF.into()],
        held: held.iter().copied().collect(),
        handoffs: handoffs.to_vec(),
        ..Default::default()
    };
    let grant = |worker: &str| {
        let mut s = c.shared.lock().unwrap();
        let now = s.now();
        let work = Work::Job { step: "unit".into(), targets: vec![("6-1-1".into(), "k5".into())] };
        s.leases.grant_at(worker, work, now, T0)
    };
    let id = grant("m4");
    c.member_beat(&beat("m4", &[id], &[]));
    c.member_beat(&beat("m2", &[], &[]));
    // A publish frees `name` a second after m4's job found it on the NAS; every cursor passes it.
    let (b2, _) = to(&m1, &[("base/6-1-1", "base/6-1-1.k9.base")], 2);
    q.batch(&b2, T0 + 1);
    let freed = T0 + 1;
    let cursors: BTreeMap<String, Cursor> =
        [("server/m6".to_string(), Cursor { seq: 2, heard: freed, at: freed, ..Default::default() })].into();
    let h = |now: u64, lease: Option<u64>| Holders { cursors: &cursors, oldest_lease: lease, now };
    assert!(q.due(&h(freed + GRACE_S - 1, None)).is_empty(), "within the grace");
    assert!(q.due(&h(freed + GRACE_S, Some(T0))).is_empty(), "m4's live lease, granted before it was freed");
    // m4 asleep: the lead releases its lease, m2 beating all the while; the grace passed, it's due.
    c.shared.lock().unwrap().leases.drop_where(|l| l.id == id);
    c.member_beat(&beat("m2", &[], &[]));
    let now = freed + GRACE_S;
    let mark = lease(c.lease_bounds().1);
    assert_eq!(q.chunk(&h(now, None), mark, 200, &|_| true).unwrap(), [name]);
    q.gone(&[name.to_string()], &[], now, mark).unwrap();
    assert_eq!(q.deletions_kept(), 1, "kept: m4 may still hand off under its lease");
    // m4 unheard past FORGET_S: the lead knows only m2 now; its lease is still owed, the mark kept.
    q.leases(["m2".to_string()].into(), None);
    assert_eq!(q.deletions_kept(), 1, "kept while m4 is forgotten");
    // m4 back three hours on, its hand-off (reusing `name`) listed: refused, its targets done again.
    let later = T0 + 3 * GRACE_S;
    let reused = Entry {
        job: JobId { epoch: 3, n: id },
        handoff: Handoff { reused: vec![name.to_string()], ..Default::default() },
        ..entry(5, "base/6-1-1.k5.base")
    };
    c.member_beat(&beat("m4", &[], &[reused.key().unwrap()]));
    let why = refusal(&q, &reused, later).expect("refused");
    assert!(why.contains(name), "{why}");
    let redo = Entry { job: JobId { epoch: 3, n: grant("m2") }, ..reused.clone() };
    assert!(refusal(&q, &redo, later).is_none(), "the redo merges");
}
