//! Targets run on small grants (jobrt::control::on_grant) finish, the same bytes whatever the grant.
//! 5 Nov: every terrain-hi job of the pool stalled at 0 tiles on a 1-thread grant. A target's
//! sampling (crate::terrain_base's chunks, threads of the target's own, which it joins) put its
//! passes on the target's own pool, whose one thread was the target waiting for them.

use crate::terrain_pack::{fill_of, make_piece, native_of, piece_levels, Fills, RawTiles, Sources};
use std::time::Duration;

/// How long a target here may take on any grant: a hang fails the test at this, not the test run's
/// own timeout.
const BOUND: Duration = store::testkit::WATCHDOG;

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
/// a target of its own (jobrt::control::on_grant), within `BOUND` all told. What each made.
fn on_grants<R: Send>(what: &str, grants: &[usize], run: impl Fn() -> R + Sync) -> Vec<R> {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("threads");
    std::env::set_var(jobrt::control::THREADS_ENV, &f);
    let made = within(what, || {
        grants
            .iter()
            .map(|&n| {
                jobrt::control::grant(&f, n).unwrap();
                jobrt::control::on_grant(|| {
                    assert_eq!(rayon::current_num_threads(), n);
                    run()
                })
            })
            .collect()
    });
    std::env::remove_var(jobrt::control::THREADS_ENV);
    made
}

/// A pool of one GeoTIFF over 9–12°E, 44–47°N (a slope east and north), `f` given it and the
/// Terrarium tiles' folder (none there: the pool alone samples).
fn with_pool<R>(f: impl FnOnce(&crate::terrain_base::Pool, &RawTiles) -> R) -> R {
    use inputs::dem::declared::{Dataset, Format, Plan};
    let d = tempfile::tempdir().unwrap();
    let store = d.path().join("fetched");
    std::fs::create_dir_all(store.join("glo")).unwrap();
    let n = 1000usize;
    let gt = [9.0, 0.003, 0.0, 47.0, 0.0, -0.003];
    let e: Vec<f32> = (0..n * n).map(|p| 100.0 + (p % n) as f32 * 0.5 + (p / n) as f32 * 0.25).collect();
    let tif = buildstore::geotiff::write_f32(n as u32, n as u32, &e, 256, gt, 4326, None).unwrap();
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
            source: Format::Geotiff {
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
    buildstore::fetch::go_offline();
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
fn coverage(d: &std::path::Path) -> buildstore::coverage::Coverage {
    coverage_km(d, 5)
}

/// A place `km` wide at 10.5°E, 45.5°N.
fn coverage_km(d: &std::path::Path, km: u32) -> buildstore::coverage::Coverage {
    let outline = vec![format!("place:10.5,45.5,{km}")];
    buildstore::coverage::Coverage::fixture(
        &[buildstore::recipes::Recipe { id: "r".into(), name: "R".into(), outline }],
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

/// What a run of `run` did as a live target (`on_live_grants`): what it made, its threads in force
/// at its end, and the cores it kept busy (the process's CPU time over the run's wall time).
type LiveRun<R> = (R, Option<usize>, f64);

/// `run` on each of `grants` as a live target's (jobrt::control::on_grant_live, terrain-deep's): its
/// threads file written as the agent does, its grant at its start, and the grant `run`'s hook
/// writes (None: none), within `BOUND` all told.
fn on_live_grants<R: Send>(
    what: &str,
    grants: &[(usize, Option<usize>)],
    run: impl Fn(&(dyn Fn() + Sync)) -> R + Sync,
) -> Vec<LiveRun<R>> {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("threads");
    std::env::set_var(jobrt::control::THREADS_ENV, &f);
    let made = within(what, || {
        grants
            .iter()
            .map(|&(n, to)| {
                jobrt::control::grant(&f, n).unwrap();
                jobrt::control::on_grant_live(|| {
                    assert_eq!(jobrt::control::in_force(), Some(n));
                    let grown = std::sync::atomic::AtomicBool::new(false);
                    let grow = || {
                        if let Some(m) = to.filter(|_| !grown.swap(true, std::sync::atomic::Ordering::SeqCst)) {
                            jobrt::control::grant(&f, m).unwrap();
                            // (Seen by the work: the file read again, a waiting thread looking again.)
                            let seen = jobrt::control::POLL + store::waits::RECHECK;
                            std::thread::sleep(seen + std::time::Duration::from_millis(200));
                            // (sleep ok: the grow seen)
                        }
                    };
                    let (cpu, t0) = (jobrt::timings::cpu_us().unwrap_or(0), std::time::Instant::now());
                    let r = run(&grow);
                    let cpu = jobrt::timings::cpu_us().unwrap_or(0).saturating_sub(cpu) as f64 / 1e6;
                    (r, jobrt::control::in_force(), cpu / t0.elapsed().as_secs_f64())
                })
            })
            .collect()
    });
    std::env::remove_var(jobrt::control::THREADS_ENV);
    made
}

/// `inner`, `grow` called once its first tiles are ready (a grant written mid-level: the level's
/// chunks sized as it began, the rest of its sampling and making on the grant grown).
struct Grows<'a> {
    inner: &'a dyn crate::terrain_pack::RawSource,
    grow: &'a (dyn Fn() + Sync),
}

impl crate::terrain_pack::RawSource for Grows<'_> {
    fn get(&self, z: u8, x: u32, y: u32) -> anyhow::Result<(Option<Vec<u8>>, bool)> {
        self.inner.get(z, x, y)
    }
    fn prefetch_each(
        &self,
        z: u8,
        tiles: &[(u32, u32)],
        turns: &crate::terrain_pack::Turns,
        ready: crate::terrain_pack::Ready,
    ) -> anyhow::Result<usize> {
        self.inner.prefetch_each(z, tiles, turns, &|got| {
            ready(got);
            (self.grow)();
        })
    }
    fn built(&self, z: u8, tiles: &[(u32, u32)]) {
        self.inner.built(z, tiles)
    }
    fn keep(&self, z: u8, x: u32, y: u32) -> bool {
        self.inner.keep(z, x, y)
    }
    fn from(&self, z: u8, x: u32, y: u32) -> Option<Vec<String>> {
        self.inner.from(z, x, y)
    }
}

/// What a terrain-deep target made: its tiles, and the manifest's names (its deep pack and mid).
type Deep = (usize, Vec<(String, String)>);

/// `runs` given a terrain-deep target (a z10 tile's band sampled from the pool, crate::terrain_deep)
/// of a place `km` wide, its finest level cut to `k` tiles, to make in a folder of its own each
/// time, a hook its sampling calls as its first tiles are ready (`Grows`); what it made.
fn deep_target<R>(km: u32, k: usize, runs: impl FnOnce(&(dyn Fn(&(dyn Fn() + Sync)) -> Deep + Sync)) -> R) -> R {
    let d = tempfile::tempdir().unwrap();
    let cov = coverage_km(d.path(), km);
    let sets = sets(14);
    let t = crate::terrain_zoom::tile_at(10.5, 45.5, crate::terrain_deep::PACK_Z);
    let levels = few(crate::terrain_deep::levels(&cov, &sets, t), k);
    assert_eq!((levels[0].0, levels[0].1.len()), (14, k));
    let fill = fill_of(&cov, &sets, &|_, _| false);
    let native = |z: u8, x: u32, y: u32| native_of(&fill, z, x, y);
    let reaching = |z: u8, x: u32, y: u32| crate::terrain_zoom::reaching(&sets, z, x, y);
    let src = Sources { native: Some(&native), sets: &sets, ..Default::default() };
    let n = std::sync::atomic::AtomicUsize::new(0);
    with_pool(|pool, raw| {
        let once = |grow: &(dyn Fn() + Sync)| -> Deep {
            let r = d.path().join(n.fetch_add(1, std::sync::atomic::Ordering::Relaxed).to_string());
            let mut out = buildstore::out::Out::unbound(
                &store::nasfs::NasRoot::local_for_test(&r.join("root")),
                &r.join("scratch"),
            )
            .unwrap();
            // (It says how far it is in its tiles, all of its levels' at its end: task #1156.)
            let said = std::sync::Mutex::new(Vec::new());
            let progress =
                |what: &str, done: u64, total: u64| said.lock().unwrap().push((what.to_string(), done, total));
            let rep = pool
                .with_tiles(raw, |tiles| {
                    let fills = Fills::new(tiles, tiles, &fill, &reaching);
                    let grows = Grows { inner: &fills, grow };
                    crate::terrain_deep::build_target(&mut out, t, levels.clone(), &grows, &src, false, &progress)
                })
                .unwrap()
                .unwrap();
            let all = levels.iter().map(|l| l.1.len() as u64).sum::<u64>();
            assert_eq!(said.into_inner().unwrap().last(), Some(&("tiles".to_string(), all, all)));
            out.save().unwrap();
            (rep.tiles, out.manifest.into_iter().collect())
        };
        runs(&once)
    })
}

/// A terrain-deep target on a grant of 1 and of 2 threads, and live on 3 (task #1550:
/// jobrt::control::on_grant_live, as terrain-deep's jobs run it): each done within the bound, the
/// same deep pack and mid.
#[test]
fn a_terrain_deep_target_on_any_grant_finishes_the_same() {
    if !alone("grant_tests::a_terrain_deep_target_on_any_grant_finishes_the_same") {
        return;
    }
    let (made, live) = deep_target(5, 4, |once| {
        let made = on_grants("a terrain-deep target", &[1, 2], || once(&|| ()));
        (made, on_live_grants("a live terrain-deep target", &[(3, None)], once))
    });
    assert!(made[0].0 > 0 && made[0].1.len() == 2, "a deep pack and its mid: {:?}", made[0]);
    assert_eq!(made[0], made[1], "the same deep pack and mid on 1 and 2 threads");
    assert_eq!((&live[0].0, live[0].1), (&made[0], Some(3)), "the same live on 3");
}

/// Task #1550 review: a live terrain-deep target of a level of several chunks, on 1 thread, keeps
/// no more than its one core busy (its sampling's chunks, their passes on the outside pool's
/// threads, and the tiles' making all on the target's one turn); begun on 1 and grown to 3 as its
/// first tiles are ready, mid-level, it takes the grow (on 3 at its end), and makes the same deep
/// pack and mid.
#[test]
fn a_live_terrain_deep_target_keeps_to_its_grant_and_grown_mid_level_finishes_the_same() {
    if !alone("grant_tests::a_live_terrain_deep_target_keeps_to_its_grant_and_grown_mid_level_finishes_the_same") {
        return;
    }
    // (70 z14 tiles: two chunks on 1 thread, terrain_base's IN_FLIGHT of 64 a chunk.)
    let live =
        deep_target(20, 70, |once| on_live_grants("a live terrain-deep target", &[(1, None), (1, Some(3))], once));
    let (on1, grown) = (&live[0], &live[1]);
    eprintln!("on 1: {:.2} cores busy; grown to 3: {:.2}", on1.2, grown.2);
    assert!(on1.0 .0 >= 70 && on1.0 .1.len() == 2, "a deep pack and its mid: {:?}", on1.0);
    assert!(on1.2 <= 1.5, "on 1 thread: {:.2} cores busy", on1.2);
    assert_eq!((on1.1, grown.1), (Some(1), Some(3)), "the grow taken within the target");
    assert_eq!(on1.0, grown.0, "the same deep pack and mid grown mid-level");
}

/// Task #1550 review: a live target's level makes its tiles on no more threads at once than its
/// grant (the makers, as many as the Mac's cores, each on a turn of the target's), and on all of
/// them: a source whose reads of the tiles take 20 ms, on a grant of 2.
#[test]
fn a_live_levels_tiles_are_made_on_no_more_threads_than_its_grant() {
    if !alone("grant_tests::a_live_levels_tiles_are_made_on_no_more_threads_than_its_grant") {
        return;
    }
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Counting {
        now: AtomicUsize,
        most: AtomicUsize,
        tile: Vec<u8>,
    }
    impl crate::terrain_pack::RawSource for Counting {
        fn get(&self, _z: u8, _x: u32, _y: u32) -> anyhow::Result<(Option<Vec<u8>>, bool)> {
            let n = self.now.fetch_add(1, Ordering::SeqCst) + 1;
            self.most.fetch_max(n, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(20)); // (sleep ok: a read's work)
            self.now.fetch_sub(1, Ordering::SeqCst);
            Ok((Some(self.tile.clone()), false))
        }
        fn prefetch_each(
            &self,
            _z: u8,
            tiles: &[(u32, u32)],
            _turns: &crate::terrain_pack::Turns,
            ready: crate::terrain_pack::Ready,
        ) -> anyhow::Result<usize> {
            ready(tiles);
            Ok(0)
        }
    }
    let raw = Counting {
        now: AtomicUsize::new(0),
        most: AtomicUsize::new(0),
        tile: roadcore::grid::encode_terrain_png(&vec![100.0; 65536], 256, 256).unwrap(),
    };
    let src = Sources::default();
    let tiles: Vec<(u32, u32)> = (0..24).map(|x| (x, 0)).collect();
    let made =
        on_live_grants("a live level", &[(2, None)], |_| make_piece(&raw, &src, vec![(12, tiles.clone())]).unwrap());
    assert!(made[0].0 .0.iter().filter(|m| m.0 == 12).count() == 24, "each tile made");
    assert_eq!(raw.most.load(Ordering::SeqCst), 2, "on its grant's 2 threads at once, no more");
}

/// Task #1550 review: a live target's sampling (crate::terrain_base's chunk threads,
/// jobrt::control::on_turn_threads, on the outside pool as wide as the Mac's cores) computes on no more threads at once than its grant, and on all
/// of them: one chunk thread whose passes (64 parallel tasks of 5 ms) the pool's other threads help
/// with, each on a turn, on a grant of 2.
#[test]
fn a_live_targets_sampling_passes_compute_on_no_more_threads_than_its_grant() {
    if !alone("grant_tests::a_live_targets_sampling_passes_compute_on_no_more_threads_than_its_grant") {
        return;
    }
    use rayon::prelude::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (now, most) = (AtomicUsize::new(0), AtomicUsize::new(0));
    on_live_grants("a live sampling", &[(2, None)], |_| {
        let turns = jobrt::control::turns();
        let pass = || {
            (0..64).into_par_iter().for_each(|_| {
                let n = now.fetch_add(1, Ordering::SeqCst) + 1;
                most.fetch_max(n, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(5)); // (sleep ok: a pass's work)
                now.fetch_sub(1, Ordering::SeqCst);
            })
        };
        jobrt::control::on_turn_threads(1, &turns, &|| {
            for _ in 0..2 {
                turns.computing(pass);
            }
            Ok(())
        })
        .unwrap()
    });
    assert_eq!(most.load(Ordering::SeqCst), 2, "on its grant's 2 threads at once, no more");
}
