//! What the lead's coordinator answers its clients from (docs/maplog.md §5.1): the map now, the
//! batches after some `seq` it still holds (in memory, `HELD_BYTES` at most), the clients' cursors
//! (§5.4) and the whole map as a snapshot, made once per `seq` asked. The log's writer keeps it
//! (`start` when the log opens, `push` at each batch); the routes only read it, never the NAS or
//! the lead's disk.
//!
//! `Changes` is the wire answer of `GET /map/changes`, which a client (crate::map::Client's users)
//! parses back: `{"reset": <seq>}`, or `{"head", "more", "batches"}`.

use super::{Batch, Cursor, Map, Mark, Snapshot};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The batches held to answer from: the newest, this many bytes of their JSON at most.
pub const HELD_BYTES: usize = 64 << 20;
/// One answer's batches: this many bytes at most (`more` past that), one batch whatever its size.
pub const ANSWER_BYTES: usize = 4 << 20;
/// The longest a `changes` request waits for a batch (`?wait=`).
pub const WAIT_S: u64 = 30;

#[derive(Default)]
struct Inner {
    map: Option<Arc<Map>>,
    /// Every batch after this `seq` is held (`recent`): a client at or past it is answered batches.
    base: u64,
    /// The batches held, oldest first: each `seq` and its JSON.
    recent: VecDeque<(u64, Arc<[u8]>)>,
    bytes: usize,
    cursors: BTreeMap<String, Cursor>,
    /// The snapshot last made (`snapshot`), and its `seq`.
    snap: Option<(u64, Arc<Vec<u8>>)>,
    /// The log's marks (super::log::Log::marks) as its writer last said.
    marks: BTreeMap<String, Mark>,
}

/// The lead's map, as its routes answer it.
#[derive(Default)]
pub struct Feed {
    inner: Mutex<Inner>,
    /// Moved on at each `start` and `push`: what a long poll waits on.
    gen: AtomicU64,
}

/// What a `changes` request is answered (`Feed::changes`).
#[derive(Debug, PartialEq)]
pub enum Changes {
    /// No map yet (the log not open): asked again later.
    NotReady,
    /// The client's `since` is older than the batches held, or ahead of the head: it takes the
    /// snapshot at this `seq` (`GET /map/snapshot`).
    Reset(u64),
    /// The batches after `since` (none when it's the head), as the answer's JSON.
    Batches { head: u64, n: usize, body: Vec<u8> },
}

/// `GET /map/changes`' answer as a client reads it.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Answer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset: Option<u64>,
    #[serde(default)]
    pub head: u64,
    #[serde(default)]
    pub more: bool,
    #[serde(default)]
    pub batches: Vec<Batch>,
}

impl std::fmt::Debug for Feed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Feed({:?})", self.head())
    }
}

impl Feed {
    pub fn new() -> Arc<Feed> {
        Arc::new(Feed::default())
    }

    /// The log opened (or seeded): its map, the batches after `base` it replayed (`held`, in turn),
    /// and the cursors its snapshot kept. What was held before goes (the log was opened again).
    pub fn start(&self, map: Arc<Map>, base: u64, held: &[Batch], cursors: BTreeMap<String, Cursor>) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut kept = std::mem::take(&mut g.cursors);
        for (k, c) in cursors {
            // (A cursor heard since the snapshot was written kept as heard.)
            kept.entry(k).and_modify(|h| *h = if h.heard >= c.heard { h.clone() } else { c.clone() }).or_insert(c);
        }
        let marks = std::mem::take(&mut g.marks);
        *g = Inner { map: Some(map), base, cursors: kept, marks, ..Default::default() };
        for b in held {
            hold(&mut g, b);
        }
        drop(g);
        self.gen.fetch_add(1, Ordering::SeqCst);
    }

    /// Batch `b` appended, the map after it `map`.
    pub fn push(&self, map: Arc<Map>, b: &Batch) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.map = Some(map);
        hold(&mut g, b);
        drop(g);
        self.gen.fetch_add(1, Ordering::SeqCst);
    }

    /// The log's marks now (its writer's, at each `start` and `push`).
    pub fn set_marks(&self, marks: BTreeMap<String, Mark>) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).marks = marks;
    }

    /// The log's marks as last set.
    pub fn marks(&self) -> BTreeMap<String, Mark> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).marks.clone()
    }

    /// Moves on at each change: a long poll answers once it has.
    pub fn gen(&self) -> u64 {
        self.gen.load(Ordering::SeqCst)
    }

    /// The map's `seq` now, if it has one.
    pub fn head(&self) -> Option<u64> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).map.as_ref().map(|m| m.seq)
    }

    /// The map now.
    pub fn map(&self) -> Option<Arc<Map>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).map.clone()
    }

    /// The answer to a client at `since` (`client` its id, its cursor noted as heard at `now`,
    /// seconds since 1970): never waits (the route's long poll asks again when `gen` moves).
    pub fn changes(&self, since: u64, client: Option<&str>, now: u64) -> Changes {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let Some(head) = g.map.as_ref().map(|m| m.seq) else { return Changes::NotReady };
        if let Some(c) = client.filter(|c| !c.is_empty() && c.len() <= 120) {
            // (When it was first heard at `since`: its adoption of it.)
            let at = g.cursors.get(c).filter(|k| k.seq == since && k.at > 0).map_or(now, |k| k.at);
            g.cursors.insert(c.to_string(), Cursor { seq: since, heard: now, at });
        }
        if since > head || since < g.base {
            return Changes::Reset(head);
        }
        let mut body = Vec::with_capacity(256);
        let mut n = 0;
        body.extend_from_slice(b"{\"batches\":[");
        let mut more = false;
        for (seq, b) in g.recent.iter().filter(|(s, _)| *s > since) {
            if n > 0 && body.len() + b.len() > ANSWER_BYTES {
                more = true;
                break;
            }
            if n > 0 {
                body.push(b',');
            }
            body.extend_from_slice(b);
            n += 1;
            debug_assert!(*seq > since);
        }
        body.extend_from_slice(format!("],\"head\":{head},\"more\":{more}}}").as_bytes());
        Changes::Batches { head, n, body }
    }

    /// The clients' cursors as last heard (saved with the log's snapshot).
    pub fn cursors(&self) -> BTreeMap<String, Cursor> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).cursors.clone()
    }

    /// The whole map now as a snapshot's bytes (zstd), and its `seq`: made once per `seq` (outside
    /// the lock: the map is shared, never copied), kept for the next client.
    pub fn snapshot(&self) -> anyhow::Result<Option<(u64, Arc<Vec<u8>>)>> {
        let map = {
            let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let Some(m) = g.map.clone() else { return Ok(None) };
            if let Some(s) = g.snap.as_ref().filter(|s| s.0 == m.seq) {
                return Ok(Some(s.clone()));
            }
            m
        };
        let b = Arc::new(Snapshot::of(&map, &BTreeMap::new()).encode()?);
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if g.snap.as_ref().is_none_or(|s| s.0 < map.seq) {
            g.snap = Some((map.seq, b.clone()));
        }
        Ok(Some((map.seq, b)))
    }
}

/// `b` held, the oldest let go past `HELD_BYTES` (`base` moving up to them).
fn hold(g: &mut Inner, b: &Batch) {
    let line: Arc<[u8]> = serde_json::to_vec(b).expect("a batch serializes").into();
    g.bytes += line.len();
    g.recent.push_back((b.seq, line));
    while g.bytes > HELD_BYTES && g.recent.len() > 1 {
        let (seq, l) = g.recent.pop_front().expect("more than one");
        g.bytes -= l.len();
        g.base = seq;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn chain(k: u64) -> (Map, Vec<Batch>) {
        let start = Map::new(1, "t1".into(), [("u/6/0/0".to_string(), json!(true))].into());
        let mut m = start.clone();
        let mut out = Vec::new();
        for s in 2..=k {
            let mut next = m.entries().clone();
            next.insert(format!("u/7/{s}/0"), json!(true));
            let b = m.batch_to(&next, s, &format!("t{s}"), "", "");
            m = m.apply(&b).unwrap();
            out.push(b);
        }
        (start, out)
    }

    fn answer(c: Changes) -> Answer {
        match c {
            Changes::Batches { body, .. } => serde_json::from_slice(&body).unwrap(),
            Changes::Reset(s) => Answer { reset: Some(s), ..Default::default() },
            Changes::NotReady => panic!("not ready"),
        }
    }

    #[test]
    fn a_client_is_answered_what_follows_its_seq_and_its_cursor_noted() {
        let f = Feed::new();
        assert_eq!(f.changes(0, None, 0), Changes::NotReady);
        let (start, bs) = chain(5);
        let mut m = start.clone();
        f.start(Arc::new(start.clone()), 1, &[], BTreeMap::new());
        for b in &bs {
            m = m.apply(b).unwrap();
            f.push(Arc::new(m.clone()), b);
        }
        let a = answer(f.changes(3, Some("server/m4"), 77));
        assert_eq!((a.head, a.more, a.batches.iter().map(|b| b.seq).collect::<Vec<_>>()), (5, false, vec![4, 5]));
        assert_eq!(f.cursors()["server/m4"], Cursor { seq: 3, heard: 77, at: 77 });
        // (Heard again at the same seq: adopted when first heard there.)
        f.changes(3, Some("server/m4"), 80);
        assert_eq!(f.cursors()["server/m4"], Cursor { seq: 3, heard: 80, at: 77 });
        // At the head: none. Before the oldest held, or past the head: reset.
        assert!(answer(f.changes(5, None, 0)).batches.is_empty());
        assert_eq!(answer(f.changes(0, None, 0)).reset, Some(5));
        assert_eq!(answer(f.changes(9, None, 0)).reset, Some(5));
        // The snapshot is the map.
        let (seq, b) = f.snapshot().unwrap().unwrap();
        assert_eq!((seq, Snapshot::decode(&b).unwrap().0), (5, m));
    }
}
