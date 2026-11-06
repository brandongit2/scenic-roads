//! Targets run on small grants (crate::control::on_grant) finish, the same bytes whatever the grant.
//! 5 Nov: every terrain-hi job of the pool stalled at 0 tiles on a 1-thread grant. A target's
//! sampling (crate::terrain_base's chunks, threads of the target's own, which it joins) put its
//! passes on the target's own pool, whose one thread was the target waiting for them.

use crate::terrain_pack::{fill_of, make_piece, native_of, piece_levels, Fills, RawTiles, Sources};
use std::time::Duration;

/// How long a target here may take on any grant: a hang fails the test at this, not the test run's
/// own timeout.
const BOUND: Duration = Duration::from_secs(300);

/// Whether this is the test's process of its own (the threads file's variable and the target under
/// way are the process's): run there, by name, its result asserted here, when not.
pub(crate) fn alone(name: &str) -> bool {
    const ALONE: &str = "SCENIC_TEST_ALONE";
    if std::env::var_os(ALONE).is_some() {
        return true;
    }
    let out = store::spawn::output(
        store::spawn::command(std::env::current_exe().unwrap()).args(["--exact", name]).env(ALONE, "1"),
    )
    .unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success() && said.contains(" 1 passed"), "{said}{}", String::from_utf8_lossy(&out.stderr));
    false
}

/// `f`, within `BOUND`: past it, this process ends failing (threads waiting on each other can't be
/// stopped), saying `what` waited.
pub(crate) fn within<R>(what: &str, f: impl FnOnce() -> R) -> R {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let what = what.to_string();
    std::thread::spawn(move || {
        if rx.recv_timeout(BOUND) == Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
            eprintln!("{what}: not done in {} s: waiting forever", BOUND.as_secs());
            std::process::exit(1);
        }
    });
    let r = f();
    drop(tx);
    r
}

/// `run` on each of `grants` as its job's grant (its threads file written as the agent does), each
/// a target of its own (crate::control::on_grant), within `BOUND` all told. What each made.
fn on_grants<R: Send>(what: &str, grants: &[usize], run: impl Fn() -> R + Sync) -> Vec<R> {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("threads");
    std::env::set_var(crate::control::THREADS_ENV, &f);
    let made = within(what, || {
        grants
            .iter()
            .map(|&n| {
                crate::control::grant(&f, n).unwrap();
                crate::control::on_grant(|| {
                    assert_eq!(rayon::current_num_threads(), n);
                    run()
                })
            })
            .collect()
    });
    std::env::remove_var(crate::control::THREADS_ENV);
    made
}

/// A pool of one GeoTIFF over 9–12°E, 44–47°N (a slope east and north), `f` given it and the
/// Terrarium tiles' folder (none there: the pool alone samples).
fn with_pool<R>(f: impl FnOnce(&crate::terrain_base::Pool, &RawTiles) -> R) -> R {
    use crate::dem::declared::{Dataset, Plan, Source};
    let d = tempfile::tempdir().unwrap();
    let store = d.path().join("fetched");
    std::fs::create_dir_all(store.join("glo")).unwrap();
    let n = 1000usize;
    let gt = [9.0, 0.003, 0.0, 47.0, 0.0, -0.003];
    let e: Vec<f32> = (0..n * n).map(|p| 100.0 + (p % n) as f32 * 0.5 + (p / n) as f32 * 0.25).collect();
    let tif = crate::geotiff::write_f32(n as u32, n as u32, &e, 256, gt, 4326, None).unwrap();
    std::fs::write(store.join("glo/T_DEM.tif"), tif).unwrap();
    let area = [9.0, 44.0, 12.0, 47.0];
    let plan = Plan {
        fmt: 1,
        version: "test".into(),
        tag: "t".into(),
        datasets: vec![Dataset {
            id: "glo".into(),
            code: 3,
            resolution: 78.0,
            cover: Some(vec![area]),
            source: Source::Geotiff {
                crs: 4326,
                read_resolution: None,
                store: true,
                files: vec![("https://e.org/T/T_DEM.tif".into(), Some(area))],
                masks: vec![None],
            },
            convert: None,
            outline: None,
        }],
    };
    let pool = crate::terrain_base::Pool::of(plan, store::nasfs::NasPath::local_for_test(store), area);
    let raw = RawTiles::new(&d.path().join("local"), crate::terrain_pack::TEST_BASE);
    crate::fetch::go_offline();
    f(&pool, &raw)
}

/// The declared datasets: the pool's one, its finest zoom `z` everywhere.
fn sets(z: u8) -> Vec<crate::terrain_zoom::Set> {
    vec![crate::terrain_zoom::Set {
        id: "glo".into(),
        line: "glo".into(),
        ..crate::terrain_zoom::test_world(z).remove(0)
    }]
}

/// A place 5 km wide at 10.5°E, 45.5°N.
fn coverage(d: &std::path::Path) -> crate::coverage::Coverage {
    crate::coverage::Coverage::fixture(
        &[crate::agent::recipes::Recipe {
            id: "r".into(),
            name: "R".into(),
            outline: vec!["place:10.5,45.5,5".into()],
        }],
        None,
        d,
    )
    .unwrap()
}

/// `levels` (finest first) cut to `k` tiles of the finest and the coarser tiles above them: a
/// target's sampling as it runs, in seconds.
fn few(levels: Vec<(u8, Vec<(u32, u32)>)>, k: usize) -> Vec<(u8, Vec<(u32, u32)>)> {
    let mut out: Vec<(u8, Vec<(u32, u32)>)> = Vec::new();
    for (z, ts) in levels {
        let keep: Vec<(u32, u32)> = match out.last() {
            None => ts.into_iter().take(k).collect(),
            Some((fz, fine)) => {
                let s = fz - z;
                ts.into_iter().filter(|&(x, y)| fine.iter().any(|&(a, b)| (a >> s, b >> s) == (x, y))).collect()
            }
        };
        out.push((z, keep));
    }
    out
}

/// A terrain-hi target (a z6 tile's levels sampled from the pool by the rule, crate::terrain_pack)
/// on a grant of 1 and of 2 threads: done within the bound, the same tiles.
#[test]
fn a_terrain_hi_target_on_one_or_two_threads_finishes_the_same() {
    if !alone("grant_tests::a_terrain_hi_target_on_one_or_two_threads_finishes_the_same") {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let cov = coverage(d.path());
    let sets = sets(12);
    let t = crate::terrain_zoom::tile_at(10.5, 45.5, 6);
    let levels = few(piece_levels(&cov, &sets, t), 4);
    assert_eq!(levels[0].0, 12);
    assert_eq!(levels[0].1.len(), 4);
    let fill = fill_of(&cov, &sets, &|_, _| false);
    let native = |z: u8, x: u32, y: u32| native_of(&fill, z, x, y);
    let reaching = |z: u8, x: u32, y: u32| crate::terrain_zoom::reaching(&sets, z, x, y);
    let src = Sources { native: Some(&native), sets: &sets, ..Default::default() };
    let made = with_pool(|pool, raw| {
        on_grants("a terrain-hi target", &[1, 2], || {
            pool.with_tiles(raw, |tiles| {
                let fills = Fills::new(tiles, tiles, &fill, &reaching);
                make_piece(&fills, &src, levels.clone()).unwrap().0
            })
            .unwrap()
        })
    });
    assert!(
        made[0].iter().filter(|m| m.0 == 12).count() == 4,
        "{:?}",
        made[0].iter().map(|m| (m.0, m.1, m.2)).collect::<Vec<_>>()
    );
    assert!(made[0] == made[1], "the same tiles on 1 and 2 threads");
}

/// A terrain-deep target (a z10 tile's band sampled from the pool, crate::terrain_deep) on a grant
/// of 1 and of 2 threads: done within the bound, the same deep pack and mid.
#[test]
fn a_terrain_deep_target_on_one_or_two_threads_finishes_the_same() {
    if !alone("grant_tests::a_terrain_deep_target_on_one_or_two_threads_finishes_the_same") {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let cov = coverage(d.path());
    let sets = sets(14);
    let t = crate::terrain_zoom::tile_at(10.5, 45.5, crate::terrain_deep::PACK_Z);
    let levels = few(crate::terrain_deep::levels(&cov, &sets, t), 4);
    assert_eq!(levels[0].0, 14);
    let fill = fill_of(&cov, &sets, &|_, _| false);
    let native = |z: u8, x: u32, y: u32| native_of(&fill, z, x, y);
    let reaching = |z: u8, x: u32, y: u32| crate::terrain_zoom::reaching(&sets, z, x, y);
    let src = Sources { native: Some(&native), sets: &sets, ..Default::default() };
    let k = std::sync::atomic::AtomicUsize::new(0);
    let made = with_pool(|pool, raw| {
        on_grants("a terrain-deep target", &[1, 2], || {
            let r = d.path().join(k.fetch_add(1, std::sync::atomic::Ordering::Relaxed).to_string());
            let mut out =
                crate::out::Out::unbound(&store::nasfs::NasRoot::local_for_test(&r.join("root")), &r.join("scratch"))
                    .unwrap();
            let rep = pool
                .with_tiles(raw, |tiles| {
                    let fills = Fills::new(tiles, tiles, &fill, &reaching);
                    crate::terrain_deep::build_target(&mut out, t, levels.clone(), &fills, &src, false)
                })
                .unwrap()
                .unwrap();
            (rep.tiles, out.manifest.into_iter().collect::<Vec<_>>())
        })
    });
    assert!(made[0].0 > 0 && made[0].1.len() == 2, "a deep pack and its mid: {:?}", made[0]);
    assert_eq!(made[0], made[1], "the same deep pack and mid on 1 and 2 threads");
}
