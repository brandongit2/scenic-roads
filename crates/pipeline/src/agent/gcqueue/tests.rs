use super::*;
use crate::handoff::Handoff;
use crate::pool::journal;
use crate::pool::nas::Mem;
use crate::pool::records::{self, Records};
use serde_json::json;
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
        term: 2,
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
    q.open(d.path(), &m1, T0).unwrap();
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
    back.open(d.path(), &q_map(&m2, &b3), T0 + 40).unwrap();
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
    q.open(d.path(), &m1, T0).unwrap();
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

fn entry(n: u64, content: &str) -> Entry {
    let h = Handoff {
        changes: [("base/6-1-1".to_string(), Some(content.to_string()))].into(),
        pending: [(content.to_string(), "sha".to_string())].into(),
        done: Some(("unit".into(), vec![("6-1-1".into(), format!("k{n}"))])),
        ..Default::default()
    };
    Entry {
        member: "m-000000000000000b".into(),
        lease: LeaseId { term: 2, n },
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
    q.open(d.path(), &map(1, &[]), T0).unwrap();
    let name = "base/6-1-1.k0.base";
    // (Deleted while leases up to 2-7 were granted.)
    q.deleted(&[name.to_string()], T0 + 100, lease(LeaseId { term: 2, n: 7 }));
    let nas = Mem::default();
    let mut r = Records { term: 2, ..Default::default() };
    let check = |e: &Entry, _: &Records| refusal(&q, e).map_or(Ok(()), Err);
    let reused = journal::write(&nas, &entry(5, name)).unwrap();
    let m = records::merge(&nas, &mut r, &[reused.clone()], &check);
    assert_eq!(m.refused.len(), 1, "{m:?}");
    assert!(m.refused[0].1.contains(name), "{}", m.refused[0].1);
    assert_eq!(r.keys.recorded("unit", "6-1-1"), None, "offered again");
    assert!(!r.manifest.contains_key("base/6-1-1"));
    // The redo, its lease granted after the deletion (its file uploaded afresh): merged.
    let redo = journal::write(&nas, &entry(8, name)).unwrap();
    let m = records::merge(&nas, &mut r, &[redo], &check);
    assert_eq!(m.applied.len(), 1, "{m:?}");
    assert_eq!(r.manifest["base/6-1-1"], name);
    // A name not deleted: no refusal. And the deletion survives a restart.
    assert!(refusal(&q, &entry(3, "base/6-1-1.k1.base")).is_none());
    let back = GcQueue::default();
    back.open(d.path(), &map(1, &[]), T0 + 200).unwrap();
    assert!(refusal(&back, &entry(5, name)).is_some());
    assert!(refusal(&back, &entry(8, name)).is_none());
}

/// A last line cut short is dropped (and cut off the file); a damaged earlier line is an error.
#[test]
fn a_torn_last_line_is_dropped() {
    let d = tempfile::tempdir().unwrap();
    let m1 = map(1, &[("a", "a.1.pack"), ("b", "b.1.pack")]);
    let q = GcQueue::default();
    q.open(d.path(), &m1, T0).unwrap();
    super::records(&q, &records(1, &[]), T0);
    let (b2, m2) = to(&m1, &[("a", "a.2.pack"), ("b", "b.1.pack")], 2);
    q.batch(&b2, T0);
    let p = d.path().join(FILE);
    let whole = std::fs::read(&p).unwrap();
    let mut torn = whole.clone();
    torn.extend_from_slice(b"{\"q\":\"b.1.pa");
    std::fs::write(&p, &torn).unwrap();
    let back = GcQueue::default();
    back.open(d.path(), &m2, T0).unwrap();
    assert!(back.queued("a.1.pack").is_some());
    assert_eq!(std::fs::read(&p).unwrap(), whole);
    let mut bad = whole.clone();
    bad[2] = b'#';
    bad.extend_from_slice(b"{\"taken\":\"x\"}\n");
    std::fs::write(&p, &bad).unwrap();
    assert!(GcQueue::default().open(d.path(), &m2, T0).is_err());
}

/// The shadow's check holds unless a due name is held (records, the newest catalogue); names kept
/// for today's window alone are counted, not warned of (they're expected every day).
#[test]
fn the_shadow_s_check_says_what_today_s_gc_kept() {
    let mut c = ShadowCheck { due: 2, removed: vec!["a".into()], gone: vec!["b".into()], ..Default::default() };
    assert!(c.holds());
    for w in ["catalogue", "young", "not swept"] {
        c.kept.insert(w.into(), vec!["w".into()]);
    }
    assert!(c.holds(), "{}", c.said());
    assert!(c.said().contains("kept for its window: 1 catalogue, 1 not swept, 1 young") && !c.said().contains("HELD"));
    c.kept.insert("records".into(), vec!["c".into()]);
    assert!(!c.holds());
    assert!(c.said().contains("HELD (a holder the queue missed): 1 records (e.g. c)"), "{}", c.said());
    c.kept.remove("records");
    c.kept.insert("map".into(), vec!["m".into()]);
    assert!(!c.holds());
}
