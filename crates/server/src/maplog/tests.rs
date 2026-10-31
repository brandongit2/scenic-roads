use super::*;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use store::catalogue::Catalogue;
use store::map::feed::Feed;
use store::map::log::Log;

/// The lead's routes (pipeline::coord::maproutes) over `feed`, on a runtime of their own, on
/// `port` (0: any); stopped when dropped, its connections cut.
struct FakeLead {
    rt: Option<tokio::runtime::Runtime>,
    port: u16,
}

impl FakeLead {
    fn on(port: u16, feed: Arc<Feed>) -> FakeLead {
        let l = store::spawn::listen(("127.0.0.1", port)).unwrap();
        l.set_nonblocking(true).unwrap();
        let port = l.local_addr().unwrap().port();
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.spawn(async move {
            let l = tokio::net::TcpListener::from_std(l).unwrap();
            let app = pipeline::coord::maproutes::router::<()>(Arc::new(move || Some(feed.clone())));
            axum::serve(l, app).await.ok();
        });
        FakeLead { rt: Some(rt), port }
    }

    fn url(&self) -> String {
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

fn config(url: String, cache: &Path, wait_s: u64) -> Config {
    Config {
        cache: Some(cache.to_owned()),
        lead: Lead::At { urls: vec![url], token: "k".into() },
        wait_s,
        retry: Duration::from_millis(50),
        client: "server/test".into(),
        only_on_the_lead: false,
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
/// (pipeline::agent::publish::Writer).
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

/// A server follows a sequence of publishes from the lead's log alone (no catalogue on its NAS)
/// and serves the same tiles, byte for byte, as a server reading the catalogues; it follows each
/// within a second of its batch.
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
    store::catalogue::write_copy(&nas_a.path().join("catalog"), &first).unwrap();
    append(&mut log, &feed, &first);
    let lead = FakeLead::on(0, feed.clone());
    // A: the catalogues. B: the log, its NAS without catalogues.
    let a = data(homes[0].path(), nas_a.path());
    let b = data(homes[1].path(), nas_b.path());
    b.set_serve_map(true);
    let cfg = config(lead.url(), cache.path(), 30);
    let stop = cfg.stop.clone();
    spawn(b.clone(), cfg);
    for k in 1..=4 {
        if k > 1 {
            let c = publish(&[nas_a.path(), nas_b.path()], k);
            store::catalogue::write_copy(&nas_a.path().join("catalog"), &c).unwrap();
            assert!(a.refresh_catalogue());
            let t = Instant::now();
            append(&mut log, &feed, &c);
            let b2 = b.clone();
            tokio::task::spawn_blocking(move || until("B follows the publish", || b2.catalogue().n == k))
                .await
                .unwrap();
            store::testkit::ended_before(t, Duration::from_secs(1), "followed within a second");
        } else {
            let b2 = b.clone();
            tokio::task::spawn_blocking(move || until("B takes the lead's map", || b2.catalogue().n == 1))
                .await
                .unwrap();
        }
        assert_eq!(a.catalogue().n, k);
        assert_eq!(*b.catalogue(), *a.catalogue(), "the map's catalogue is the catalogue {k}");
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
    b.set_serve_map(true);
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

/// The lead away: a server with no map takes the NAS's snapshot; with none there either, it serves
/// the catalogues as before; a map it holds is kept, a newer NAS snapshot taken.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_nas_snapshot_is_used_while_the_lead_is_down() {
    let (nas, home, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root = store::nasfs::NasRoot::local_for_test(nas.path().to_owned());
    let one = publish(&[nas.path()], 1);
    store::catalogue::write_copy(&nas.path().join("catalog"), &one).unwrap();
    let b = data(home.path(), nas.path());
    b.set_serve_map(true);
    // (No lead: a port nothing listens on.)
    let dead = store::spawn::listen("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut f = Follower::new(b.clone(), config(format!("http://127.0.0.1:{dead}"), cache.path(), 0));
    // Neither the lead nor a snapshot: the catalogues.
    assert!(!f.round());
    assert_eq!((f.seq(), b.catalogue().n), (None, 1));
    // A snapshot on the NAS, newer: taken and served.
    let three = publish(&[nas.path()], 3);
    store::map::write_nas_snapshot(&root, &Map::of(&three).unwrap()).unwrap();
    let mut f = Follower::new(b.clone(), config(format!("http://127.0.0.1:{dead}"), cache.path(), 0));
    assert!(!f.round());
    assert_eq!((f.seq(), b.catalogue().n), (Some(3), 3));
    assert_eq!(*b.catalogue(), three);
    let want = {
        let a = data(tempfile::tempdir().unwrap().path(), nas.path());
        store::catalogue::write_copy(&nas.path().join("catalog"), &three).unwrap();
        assert!(a.refresh_catalogue());
        tile(&a, home.path()).await
    };
    assert_eq!(tile(&b, home.path()).await, want);
    // Kept on this Mac (its first save): a server started again away from both starts from it.
    let (nas2, home2) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let c = data(home2.path(), nas2.path());
    c.set_serve_map(true);
    let mut g = Follower::new(c.clone(), config(format!("http://127.0.0.1:{dead}"), cache.path(), 0));
    g.start();
    assert_eq!((g.seq(), c.catalogue().n), (Some(3), 3));
}

/// Shadow (the switch off): the map is followed and checked, the catalogues served.
#[test]
fn in_shadow_the_catalogues_are_served() {
    let (nas, home, lead_dir, cache) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let feed = Feed::new();
    let mut log = Log::open(lead_dir.path()).unwrap();
    let one = publish(&[nas.path()], 1);
    store::catalogue::write_copy(&nas.path().join("catalog"), &one).unwrap();
    append(&mut log, &feed, &one);
    append(&mut log, &feed, &publish(&[nas.path()], 2));
    let lead = FakeLead::on(0, feed.clone());
    let b = data(home.path(), nas.path());
    let mut f = Follower::new(b.clone(), config(lead.url(), cache.path(), 0));
    assert!(f.round());
    assert_eq!((f.seq(), b.catalogue().n), (Some(2), 1));
    // Switched on: the map served.
    b.set_serve_map(true);
    f.hand_on();
    assert_eq!(b.catalogue().n, 2);
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
    b.set_serve_map(true);
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

/// On a Mac that isn't the declared lead nothing is followed or held; on the declared lead, it is.
#[test]
fn only_the_declared_lead_follows() {
    let (nas, home) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let b = data(home.path(), nas.path());
    let lead = |host: &str| {
        let p = nas.path().join(pipeline::pool::term::LEAD);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, serde_json::json!({ "member": "m-1", "host": host }).to_string()).unwrap();
    };
    let mut f = Follower::new(b.clone(), config("http://127.0.0.1:1".into(), home.path(), 0));
    assert!(!f.on_the_lead(), "no declaration");
    lead("another-mac");
    assert!(!f.on_the_lead());
    lead(crate::data::host());
    assert!(f.on_the_lead());
}
