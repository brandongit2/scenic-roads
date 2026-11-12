use super::*;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use store::catalogue::Catalogue;
use store::map::feed::Feed;
use store::map::log::Log;

/// The lead's routes (agentcore::coord::maproutes) over `feed`, on a runtime of their own, on
/// `port` (0: any); stopped when dropped, its connections cut.
pub(crate) struct FakeLead {
    rt: Option<tokio::runtime::Runtime>,
    port: u16,
}

impl FakeLead {
    pub(crate) fn on(port: u16, feed: Arc<Feed>) -> FakeLead {
        let l = store::spawn::listen(("127.0.0.1", port)).unwrap();
        l.set_nonblocking(true).unwrap();
        let port = l.local_addr().unwrap().port();
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.spawn(async move {
            let l = tokio::net::TcpListener::from_std(l).unwrap();
            let app = agentcore::coord::maproutes::router::<()>(Arc::new(move || Some(feed.clone())));
            axum::serve(l, app).await.ok();
        });
        FakeLead { rt: Some(rt), port }
    }

    pub(crate) fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

impl Drop for FakeLead {
    fn drop(&mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

pub(crate) fn config(url: String, cache: &Path, wait_s: u64) -> Config {
    Config {
        cache: Some(cache.to_owned()),
        lead: Lead::At { urls: vec![url], token: "k".into() },
        wait_s,
        retry: Duration::from_millis(50),
        client: "server/test".into(),
        save_every: SAVE_EVERY,
        stop: Arc::new(AtomicBool::new(false)),
    }
}

/// Catalogue `k` of a chain: the rails' root pack holds tile 0/0/0, its bytes `k`'s own, written
/// to each NAS in `nas`; units that grow with `k`.
fn publish(nas: &[&Path], k: u64) -> Catalogue {
    let tile = format!("rails tile of catalogue {k}").into_bytes();
    let content = format!("layers/rails/root/0-0-0.{k:016x}.pack");
    for n in nas {
        let path = n.join(&content);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut w = store::pack::PackWriter::create(
            &path,
            serde_json::json!({"layer": "rails", "scope": "root", "root": "0/0/0", "encoding": "mvt"}),
            false,
        )
        .unwrap();
        w.add(0, 0, 0, &tile, tile.len() as u32).unwrap();
        w.finish().unwrap();
    }
    let mut c = Catalogue::new(k);
    c.created = format!("2026-10-30T00:00:{k:02}Z");
    c.files.insert(
        "layers/rails/root/0-0-0".into(),
        store::catalogue::FileRef {
            file: content.clone(),
            size: std::fs::metadata(nas[0].join(&content)).unwrap().len(),
            ..Default::default()
        },
    );
    c.layers.insert(
        "rails".into(),
        store::catalogue::Layer {
            encoding: "mvt".into(),
            minzoom: 0,
            maxzoom: 14,
            root: Some("layers/rails/root/0-0-0".into()),
            ..Default::default()
        },
    );
    c.units = (0..k).map(|i| format!("6/{i}/0")).collect();
    c
}

/// The lead's log: `cat` appended (the first seeds it), the feed told, as the lead's writer does
/// (agentcore::agent::publish::Writer).
fn append(log: &mut Log, feed: &Feed, cat: &Catalogue) {
    if log.map().is_empty() {
        log.seed(Map::of(cat).unwrap()).unwrap();
        feed.start(log.map(), cat.n, &[], Default::default());
    } else {
        let b = log.map().batch_of(cat, "", "").unwrap();
        feed.push(log.append(&b).unwrap(), &b);
    }
}

fn data(home: &Path, nas: &Path) -> Arc<Data> {
    Data::open(crate::data::Options {
        home: home.to_owned(),
        nas_root: Some(store::nasfs::NasRoot::local_for_test(nas.to_owned())),
        mirror: false,
        reserve: 0,
    })
    .unwrap()
}

/// The rails tile 0/0/0 as the server answers it: status and bytes.
async fn tile(d: &Arc<Data>, home: &Path) -> (u16, Vec<u8>) {
    use tower::ServiceExt;
    let s = crate::test_state_from(home, d.clone());
    let app = axum::Router::new()
        .route("/tiles/rails/{z}/{x}/{y}", axum::routing::get(crate::tiles::rail_tile))
        .with_state(s);
    let r = app
        .oneshot(axum::http::Request::builder().uri("/tiles/rails/0/0/0").body(axum::body::Body::empty()).unwrap())
        .await
        .unwrap();
    let st = r.status().as_u16();
    (st, axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap().to_vec())
}

fn until(what: &str, ok: impl FnMut() -> bool) {
    store::testkit::eventually(what, ok);
}

/// A server follows a sequence of publishes from the lead's log alone (no map on its NAS) and
/// serves the same tiles, byte for byte, as a server given each map whole (a `--root`'s map); it
/// follows each by the lead's push, not its poll's end.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_follows_publishes_and_serves_byte_identical_tiles() {
    let (nas_a, nas_b, homes, lead_dir, cache) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()],
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let feed = Feed::new();
    let mut log = Log::open(lead_dir.path()).unwrap();
    let first = publish(&[nas_a.path(), nas_b.path()], 1);
    store::testkit::map_root(nas_a.path(), &first);
    append(&mut log, &feed, &first);
    let lead = FakeLead::on(0, feed.clone());
    // A: each map whole. B: the log, its NAS without a map.
    let a = data(homes[0].path(), nas_a.path());
    let b = data(homes[1].path(), nas_b.path());
    assert_eq!(b.catalogue().n, 0);
    let cfg = config(lead.url(), cache.path(), 30);
    let stop = cfg.stop.clone();
    spawn(b.clone(), cfg);
    for k in 1..=4 {
        if k > 1 {
            let c = publish(&[nas_a.path(), nas_b.path()], k);
            store::testkit::map_root(nas_a.path(), &c);
            assert!(a.serve_root_map());
            let t = Instant::now();
            append(&mut log, &feed, &c);
            let b2 = b.clone();
            tokio::task::spawn_blocking(move || until("B follows the publish", || b2.catalogue().n == k))
                .await
                .unwrap();
            // (Task #921: half its long poll's 30 s, the time it had taken without the push; no
            // "within a second" a loaded Mac can miss.)
            store::testkit::ended_before(t, Duration::from_secs(30), "followed by the push, not its poll's end");
        } else {
            let b2 = b.clone();
            tokio::task::spawn_blocking(move || until("B takes the lead's map", || b2.catalogue().n == 1))
                .await
                .unwrap();
        }
        assert_eq!(a.catalogue().n, k);
        assert_eq!(*b.catalogue(), *a.catalogue(), "the map followed is map {k}");
        let (ta, tb) = (tile(&a, homes[0].path()).await, tile(&b, homes[1].path()).await);
        assert_eq!(ta.0, 200);
        assert_eq!(ta, tb, "tile at {k}");
    }
    // The lead's feed noted B's cursor.
    assert_eq!(feed.cursors()["server/test"].seq, 4);
    stop.store(true, Ordering::SeqCst);
}

/// The lead stopped while the server's request waits on it, and started again (its log read back
/// from its disk) with a batch more: the server goes on from where it was.
#[test]
fn a_lead_restart_mid_poll() {
    let (nas, home, lead_dir, cache) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let feed = Feed::new();
    let mut log = Log::open(lead_dir.path()).unwrap();
    append(&mut log, &feed, &publish(&[nas.path()], 1));
    let lead = FakeLead::on(0, feed.clone());
    let port = lead.port;
    let b = data(home.path(), nas.path());
    let cfg = config(lead.url(), cache.path(), 30);
    let stop = cfg.stop.clone();
    spawn(b.clone(), cfg);
    until("B takes the map", || b.catalogue().n == 1);
    // (B's request waiting now: the cursor heard at 1.)
    until("B polls", || feed.cursors().get("server/test").is_some_and(|c| c.seq == 1));
    std::thread::sleep(Duration::from_millis(200)); // (sleep ok: B mid-poll when the lead stops)
    drop(lead);
    append(&mut log, &feed, &publish(&[nas.path()], 2));
    drop(log);
    // Started again: the log opened from its disk, its batch replayed into a new feed.
    let mut log = Log::open(lead_dir.path()).unwrap();
    let feed = Feed::new();
    let (base, held) = log.replayed();
    feed.start(log.map(), base, &held, log.cursors().clone());
    let _lead = FakeLead::on(port, feed.clone());
    until("B follows the restarted lead", || b.catalogue().n == 2);
    append(&mut log, &feed, &publish(&[nas.path()], 3));
    until("and its next batch", || b.catalogue().n == 3);
    stop.store(true, Ordering::SeqCst);
}

/// The lead away and no map kept here: nothing is served, whatever the NAS holds (it keeps no copy
/// of the map, task #1119); the lead answering, its map is taken and served.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_no_map_kept_nothing_is_served_until_the_lead_answers() {
    let (nas, home, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let b = data(home.path(), nas.path());
    // (No lead: a port nothing listens on.)
    let dead = store::spawn::listen("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut f = Follower::new(b.clone(), config(format!("http://127.0.0.1:{dead}"), cache.path(), 0));
    f.start();
    for _ in 0..2 {
        assert!(!f.round());
        assert_eq!((f.seq(), b.catalogue().n), (None, 0));
    }
    // The lead answers: its map taken, the tiles its own.
    let (feed, lead_dir) = (Feed::new(), tempfile::tempdir().unwrap());
    let mut log = Log::open(lead_dir.path()).unwrap();
    let three = publish(&[nas.path()], 3);
    append(&mut log, &feed, &three);
    let lead = FakeLead::on(0, feed.clone());
    let mut f = Follower::new(b.clone(), config(lead.url(), cache.path(), 0));
    assert!(f.round());
    assert_eq!((f.seq(), b.catalogue().n), (Some(3), 3));
    assert_eq!(*b.catalogue(), three);
    assert_eq!(tile(&b, home.path()).await.0, 200);
}

/// A server on demand (docs/roles.md §8.3, 7a) keeps the map each time it moves: started on the map
/// this Mac kept, it follows the lead to a newer one and keeps it at once, so a later start away
/// from the lead serves the map its downloads were copied for. A resident one keeps the newer map
/// only `SAVE_EVERY` after its start.
#[test]
fn a_server_on_demand_keeps_the_map_as_it_moves() {
    let nas = tempfile::tempdir().unwrap();
    let kept = |folder: &Path| Snapshot::decode(&std::fs::read(folder.join(SAVED)).unwrap()).unwrap().0.seq;
    let one = Map::of(&publish(&[nas.path()], 1)).unwrap();
    let (feed, lead_dir) = (Feed::new(), tempfile::tempdir().unwrap());
    let mut log = Log::open(lead_dir.path()).unwrap();
    for k in 1..=3 {
        append(&mut log, &feed, &publish(&[nas.path()], k));
    }
    let lead = FakeLead::on(0, feed.clone());
    for (save_every, want) in [(Duration::ZERO, 3), (SAVE_EVERY, 1)] {
        let (home, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::write(cache.path().join(SAVED), Snapshot::of(&one, &Default::default()).encode().unwrap()).unwrap();
        let cfg = Config { save_every, ..config(lead.url(), cache.path(), 0) };
        let mut f = Follower::new(data(home.path(), nas.path()), cfg);
        f.start();
        assert!(f.round());
        assert_eq!(f.seq(), Some(3));
        assert_eq!(kept(cache.path()), want, "kept with save_every {save_every:?}");
    }
}

/// The offline map (docs/maplog.md §5.3; kept on the laptops, owner's decision): a server started
/// with neither the lead nor the NAS serves the map this Mac kept (its cache folder's
/// `current.json.zst`), byte for byte; with none kept, nothing. No catalogue is read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn offline_the_map_kept_here_is_served() {
    let (nas, home, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let feed = Feed::new();
    let lead_dir = tempfile::tempdir().unwrap();
    let mut log = Log::open(lead_dir.path()).unwrap();
    let two = publish(&[nas.path()], 2);
    append(&mut log, &feed, &publish(&[nas.path()], 1));
    append(&mut log, &feed, &two);
    let want = {
        // (Followed once from the lead, kept on this Mac by its first save.)
        let lead = FakeLead::on(0, feed.clone());
        let b = data(home.path(), nas.path());
        let mut f = Follower::new(b.clone(), config(lead.url(), cache.path(), 0));
        assert!(f.round());
        assert_eq!(b.catalogue().n, 2);
        assert!(cache.path().join(SAVED).exists());
        tile(&b, home.path()).await
    };
    assert_eq!(want.0, 200);
    // Away from both: no lead answers, no NAS (its root gone), the mirror holding the tile's pack.
    let (home2, gone) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let pack = two.files["layers/rails/root/0-0-0"].file.clone();
    let mirrored = home2.path().join("mirror").join(&pack);
    std::fs::create_dir_all(mirrored.parent().unwrap()).unwrap();
    std::fs::copy(nas.path().join(&pack), &mirrored).unwrap();
    let away = Data::open(crate::data::Options {
        home: home2.path().to_owned(),
        nas_root: Some(store::nasfs::NasRoot::local_for_test(gone.path().join("no-nas"))),
        mirror: true,
        reserve: 0,
    })
    .unwrap();
    assert_eq!(away.catalogue().n, 0, "nothing on a NAS that isn't there");
    let dead = store::spawn::listen("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let cfg = config(format!("http://127.0.0.1:{dead}"), cache.path(), 0);
    let stop = cfg.stop.clone();
    // (Served before `spawn` returns: the map kept, read first.)
    spawn(away.clone(), cfg);
    assert_eq!(*away.catalogue(), two);
    assert_eq!(tile(&away, home2.path()).await, want);
    stop.store(true, Ordering::SeqCst);
    // None kept: nothing, the lead away.
    let (cache2, home3) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let c = data(home3.path(), nas.path());
    let mut g = Follower::new(c.clone(), config(format!("http://127.0.0.1:{dead}"), cache2.path(), 0));
    g.start();
    assert!(!g.round());
    assert_eq!((g.seq(), c.catalogue().n), (None, 0), "nothing kept here");
}

/// A lead restored behind the map held (its log at 1, the server's map at 2): the server resets to
/// its snapshot, yet the catalogue served never steps back; the next batch past it is served.
#[test]
fn a_lead_behind_the_map_never_takes_the_server_back() {
    let (nas, home, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (feed, mut log) = (Feed::new(), Log::open(dir_a.path()).unwrap());
    append(&mut log, &feed, &publish(&[nas.path()], 1));
    append(&mut log, &feed, &publish(&[nas.path()], 2));
    let b = data(home.path(), nas.path());
    {
        let lead = FakeLead::on(0, feed);
        let mut f = Follower::new(b.clone(), config(lead.url(), cache.path(), 0));
        assert!(f.round());
        assert_eq!(b.catalogue().n, 2);
    }
    let (feed, mut log) = (Feed::new(), Log::open(dir_b.path()).unwrap());
    append(&mut log, &feed, &publish(&[nas.path()], 1));
    let lead = FakeLead::on(0, feed.clone());
    let mut f = Follower::new(b.clone(), config(lead.url(), cache.path(), 0));
    f.start();
    assert_eq!(f.seq(), Some(2));
    assert!(f.round());
    assert_eq!((f.seq(), b.catalogue().n), (Some(1), 2), "reset to the lead's map, the served one kept");
    append(&mut log, &feed, &publish(&[nas.path()], 2));
    append(&mut log, &feed, &publish(&[nas.path()], 3));
    assert!(f.round());
    assert_eq!((f.seq(), b.catalogue().n), (Some(3), 3));
}
