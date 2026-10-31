//! The map's routes on the lead's coordinator (docs/maplog.md §5.1), answered from the feed the
//! lead's log writer keeps (store::map::feed, crate::agent::publish::Shadow): memory alone, never
//! the NAS or a disk.
//!
//! - `GET /map/changes?since=<seq>&client=<id>&wait=<s>`: the batches after `since`, or a reset to
//!   the snapshot; with `wait` (30 s at most) it answers once a batch comes or the wait is out.
//! - `GET /map/snapshot`: the whole map at the head (zstd JSON, store::map::Snapshot), its `seq` in
//!   `X-Map-Seq`.
//!
//! 503 while there's no map (not the lead, or its log not open yet): a client goes on as it would
//! with the lead away. `router` serves them for any state: the coordinator merges it behind its
//! gate (the build's key), a server's test serves it bare.

use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use std::sync::Arc;
use std::time::Duration;
use store::map::feed::{Changes, Feed, WAIT_S};

/// Where the routes find the feed (None: no map here).
pub type FeedOf = Arc<dyn Fn() -> Option<Arc<Feed>> + Send + Sync>;

/// How often a long poll looks whether a batch came.
const LOOK: Duration = Duration::from_millis(50);

#[derive(serde::Deserialize, Default)]
pub struct Ask {
    #[serde(default)]
    since: u64,
    #[serde(default)]
    client: Option<String>,
    #[serde(default)]
    wait: Option<u64>,
}

/// The map's routes, for a router of any state.
pub fn router<S: Clone + Send + Sync + 'static>(feed: FeedOf) -> Router<S> {
    Router::new().route("/map/changes", get(changes)).route("/map/snapshot", get(snapshot)).with_state(feed)
}

fn not_ready() -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, [(header::CACHE_CONTROL, "no-store")], "no map here").into_response()
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

async fn changes(State(feed): State<FeedOf>, Query(a): Query<Ask>) -> Response {
    let Some(f) = feed() else { return not_ready() };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(a.wait.unwrap_or(0).min(WAIT_S));
    loop {
        let gen = f.gen();
        let c = f.changes(a.since, a.client.as_deref(), unix_now());
        let body = match c {
            Changes::NotReady => return not_ready(),
            Changes::Reset(head) => format!("{{\"reset\":{head}}}").into_bytes(),
            Changes::Batches { n: 0, .. } if tokio::time::Instant::now() < deadline => {
                // (Nothing new: asked again once the feed moves, or the wait is out.)
                while f.gen() == gen && tokio::time::Instant::now() < deadline {
                    tokio::time::sleep(LOOK).await;
                }
                continue;
            }
            Changes::Batches { body, .. } => body,
        };
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "no-store")],
            body,
        )
            .into_response();
    }
}

async fn snapshot(State(feed): State<FeedOf>) -> Response {
    let Some(f) = feed() else { return not_ready() };
    match tokio::task::spawn_blocking(move || f.snapshot()).await {
        Ok(Ok(Some((seq, b)))) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/zstd".to_string()),
                (header::CACHE_CONTROL, "no-store".to_string()),
                (header::HeaderName::from_static("x-map-seq"), seq.to_string()),
            ],
            b.as_ref().clone(),
        )
            .into_response(),
        Ok(Ok(None)) => not_ready(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use crate::coord::client::Client;
    use serde_json::json;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use store::map::feed::{Answer, Feed};
    use store::map::{Batch, Map};

    /// A map at 1 and batches 2..=k, each a z6 tile more (each with the map after it).
    fn chain(k: u64) -> (Map, Vec<(Map, Batch)>) {
        let start = Map::new(1, "t1".into(), [("u/6/0/0".to_string(), json!(true))].into());
        let (mut m, mut out) = (start.clone(), Vec::new());
        for s in 2..=k {
            let mut next = m.entries().clone();
            next.insert(format!("u/6/{s}/0"), json!(true));
            let b = m.batch_to(&next, s, &format!("t{s}"), "", "");
            m = m.apply(&b).unwrap();
            out.push((m.clone(), b));
        }
        (start, out)
    }

    fn lead(d: &std::path::Path) -> (crate::coord::Coordinator, Client, Arc<Feed>) {
        let (c, port) = crate::coord::start_for_test(&d.join("a"), "m4", "");
        let w = Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "server/m1");
        let feed = Feed::new();
        c.set_maplog(feed.clone());
        (c, w, feed)
    }

    fn ask(w: &Client, path: &str) -> Answer {
        serde_json::from_slice(&w.get_bytes(path).unwrap()).unwrap()
    }

    /// The routes answer from memory: the NAS hung (every call on it held), they answer at once,
    /// and a long poll answers as soon as a batch comes.
    #[test]
    fn changes_answers_without_the_nas() {
        let d = tempfile::tempdir().unwrap();
        let (c, w, feed) = lead(d.path());
        // No map yet: 503, said as such.
        assert!(w.get_bytes("/map/changes?since=0").unwrap_err().to_string().contains("503"));
        let nas = d.path().join("nas");
        std::fs::create_dir_all(&nas).unwrap();
        c.set_root(Some(&store::nasfs::NasRoot::local_for_test(nas.clone())));
        let _hung = crate::agent::nasread::fake::hang(&nas);
        let (start, bs) = chain(4);
        feed.start(Arc::new(start), 1, &[], Default::default());
        for (m, b) in &bs[..2] {
            feed.push(Arc::new(m.clone()), b);
        }
        let t = Instant::now();
        let a = ask(&w, "/map/changes?since=1&client=server/m1");
        assert_eq!((a.head, a.batches.iter().map(|b| b.seq).collect::<Vec<_>>()), (3, vec![2, 3]));
        assert_eq!(a.batches[0], bs[0].1);
        let snap = w.get_bytes("/map/snapshot").unwrap();
        assert_eq!(store::map::Snapshot::decode(&snap).unwrap().0, bs[1].0);
        store::testkit::ended_before(t, Duration::from_secs(5), "the routes on a hung NAS");
        assert_eq!(feed.cursors()["server/m1"].seq, 1);
        // A long poll at the head: answered when the next batch comes, not at its end.
        let f2 = feed.clone();
        let (m, b) = bs[2].clone();
        let pusher = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            f2.push(Arc::new(m), &b);
        });
        let t = Instant::now();
        let a = ask(&w, "/map/changes?since=3&wait=10");
        assert_eq!(a.batches.iter().map(|b| b.seq).collect::<Vec<_>>(), vec![4]);
        store::testkit::ended_before(t, Duration::from_secs(3), "the long poll answered at the batch");
        pusher.join().unwrap();
        // None comes: answered empty at the wait's end.
        let a = ask(&w, "/map/changes?since=4&wait=1");
        assert!(a.batches.is_empty() && a.reset.is_none() && a.head == 4);
        c.stop();
    }

    /// A client older than the batches held, or ahead of the head (a restored lead), is told to
    /// reset to the snapshot.
    #[test]
    fn an_old_since_resets() {
        let d = tempfile::tempdir().unwrap();
        let (c, w, feed) = lead(d.path());
        let (_, bs) = chain(6);
        // (The log opened at 3: the batches after it held.)
        let held: Vec<Batch> = bs[2..].iter().map(|(_, b)| b.clone()).collect();
        feed.start(Arc::new(bs[4].0.clone()), bs[1].0.seq, &held, Default::default());
        assert_eq!(ask(&w, "/map/changes?since=2").reset, Some(6));
        assert_eq!(ask(&w, "/map/changes?since=0").reset, Some(6));
        assert_eq!(ask(&w, "/map/changes?since=9").reset, Some(6));
        let a = ask(&w, "/map/changes?since=3");
        assert_eq!((a.reset, a.batches.len()), (None, 3));
        // The reset's snapshot is the map at the head.
        let (m, _) = store::map::Snapshot::decode(&w.get_bytes("/map/snapshot").unwrap()).unwrap();
        assert_eq!(m, bs[4].0);
        c.stop();
    }
}
