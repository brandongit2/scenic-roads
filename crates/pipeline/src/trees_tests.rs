//! trees' tests that start a coordinator (crate::coord: trees::task's rows to workers and back) or
//! make room as the agent does (crate::agent::room): the trees crate's own tests can't (a family
//! doesn't depend on pipeline; docs/crates.md §6.2), so they're pipeline's, as terrain_tests are.
#![cfg(test)]

use crate::coord::client::{Client, LeaseCalls};
use crate::trees::task::testkit::{block_files, COV};
use crate::trees::task::{parse_blocks, parse_squares, KIND};
use crate::trees::testkit::{sources_file, squares_dir, ROWS_DIR};
use crate::trees::{block_dir, blocks_files, chm_name, mask::Shapes, z3, Inputs, Run, Source, LAYERS, MID, ZBLOCK};
use offload::Offload;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Z6 tile 6/33/22's run in `dir/out`, with `offload`.
fn piece(d: &Path, sq: &Path, out: &str, offload: Option<&Offload>) {
    std::fs::write(d.join("cov.json"), COV).unwrap();
    let a = Run {
        tile: (6, 33, 22),
        coverage: d.join("cov.json"),
        chm: sq.into(),
        chm_store: d.join("store"),
        sources: sources_file(d),
        leaf: sq.into(),
        out: d.join(out),
        keep: None,
    };
    rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap()
        .install(|| crate::trees::z6_with(&a, offload))
        .unwrap();
}

/// Whether two z6 tiles' runs wrote the same bytes.
fn same_piece(a: &Path, b: &Path) -> bool {
    LAYERS
        .iter()
        .map(|l| format!("trees-{l}.tiles"))
        .chain([MID.to_string()])
        .all(|f| std::fs::read(a.join(&f)).unwrap() == std::fs::read(b.join(&f)).unwrap())
}

/// A coordinator on a virtual clock, and the clock: the job's waits are counted in it, and a
/// worker acts at its moments (`worker`).
fn coordinator(p: &Path) -> (crate::coord::Coordinator, String, Arc<store::clock::Virtual>) {
    let (c, port) = crate::coord::start_for_test(&p.join("coord"), "m4", "");
    let clock = store::clock::Virtual::new();
    c.shared.lock().unwrap().clock = clock.clone();
    (c, format!("http://127.0.0.1:{port}"), clock)
}

/// A worker on the clock: a second from now (the job's first moment of waiting) it asks for a
/// row and, given one, runs the program's code over it as `trees --blocks` does (its squares in
/// `sq`), sends what it wrote, done; or, `hold`, holds it and
/// never says. The task it was given, once it was.
fn worker(
    clock: &store::clock::Virtual,
    url: &str,
    token: &str,
    name: &str,
    sq: PathBuf,
    home: PathBuf,
    hold: bool,
) -> Arc<Mutex<Option<serde_json::Value>>> {
    let m = Client::at(vec![url.to_string()], token.to_string(), name);
    let given = Arc::new(Mutex::new(None));
    let slot = given.clone();
    clock.after(std::time::Duration::from_secs(1), move || {
        let ask =
            crate::coord::Ask { kind: "native".into(), can: vec![KIND.into()], mem_mb: 4096, ..Default::default() };
        let Some(g) = m.ask(&ask).unwrap() else { return };
        let crate::coord::Granted::Task { task, .. } = g.work else { panic!("not a task") };
        *slot.lock().unwrap() = Some(task.clone());
        if hold {
            return;
        }
        let u = home.join("u");
        std::fs::create_dir_all(&u).unwrap();
        for i in task["inputs"].as_array().unwrap() {
            let p = i[0].as_str().unwrap();
            std::fs::write(home.join(p), m.get_bytes(&format!("/work/in/{}/{p}", g.lease)).unwrap()).unwrap();
        }
        let args: Vec<String> = task["runs"][0]["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| {
                a.as_str()
                    .unwrap()
                    .replace("{dir}", &u.to_string_lossy())
                    .replace("{sources}/trees/leaf", &sq.to_string_lossy())
                    .replace("{chm}", &sq.to_string_lossy())
            })
            .collect();
        let arg = |k: &str| args[args.iter().position(|a| a == k).unwrap() + 1].clone();
        let shapes = Shapes::parse(&std::fs::read_to_string(arg("--coverage")).unwrap()).unwrap();
        let fetch = buildstore::fetch::MapFetch::default();
        let inp = Inputs {
            chm: Source::Dir(arg("--chm").into()),
            leaf: Source::Dir(arg("--leaf").into()),
            fetch: &fetch,
            record: None,
            there: Some(parse_squares(&arg("--squares")).unwrap()),
        };
        let row = parse_blocks(&arg("--blocks")).unwrap();
        blocks_files(&shapes, &inp, &row, Path::new(&arg("--out"))).unwrap();
        let mut outputs = Vec::new();
        for &b in &row {
            for f in block_files() {
                let bytes = std::fs::read(block_dir(&u, b).join(&f)).unwrap();
                let path = format!("u/{}-{}-{}/{f}", ZBLOCK, b.0, b.1);
                m.put_bytes(&format!("/work/out/{}/{path}", g.lease), &bytes).unwrap();
                outputs.push(crate::coord::task::Output { path, size: bytes.len() as u64 });
            }
        }
        let done = crate::coord::Done { lease: g.lease, outputs, peak_mb: 400, secs: 0.5, ..Default::default() };
        assert_eq!(m.done(&done).unwrap(), crate::coord::client::Handed::Taken);
    });
    given
}

#[test]
fn a_pieces_rows_go_to_workers_and_come_back_the_same() {
    let sq = squares_dir();
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    piece(p, sq.path(), "alone", None);
    let (c, url, clock) = coordinator(p);
    let o = Offload::at(url.clone(), c.job_token.clone(), &p.join("job")).clock(clock.clone());
    let tok = c.contact.token.clone();
    let ask = crate::coord::Ask { kind: "native".into(), can: vec![KIND.into()], mem_mb: 4096, ..Default::default() };
    // No worker around: nothing offered, the same z6 tile.
    piece(p, sq.path(), "none", Some(&o));
    assert!(same_piece(&p.join("alone"), &p.join("none")));
    assert!(c.shared.lock().unwrap().tasks.by_id.is_empty());

    // A worker measured fast (a fifth of this Mac's time): its row (the last, never both)
    // waited for and taken, the same z6 tile.
    let m1 = Client::at(vec![url.clone()], tok.clone(), "m1");
    assert!(m1.ask(&ask).unwrap().is_none());
    c.shared.lock().unwrap().tasks.paces.insert(("m1".into(), KIND.into()), 0.2);
    // (Waited on until it's back, here and below: this is the exchange, not the waiting rule.)
    let patient = Offload::at(url.clone(), c.job_token.clone(), &p.join("job-patient"))
        .waiting(crate::offload::Waiting::UntilDone)
        .clock(clock.clone());
    let w = worker(&clock, &url, &tok, "m1", sq.path().into(), p.join("m1"), false);
    piece(p, sq.path(), "taken", Some(&patient));
    let task = w.lock().unwrap().take().expect("it was given the row");
    assert_eq!((task["unit"].as_str(), task["blocks"].as_str()), (Some("8/132/89"), Some("8/132/89,8/133/89")));
    assert_eq!(task["runs"][0]["prog"], "trees");
    assert!(same_piece(&p.join("alone"), &p.join("taken")));
    {
        let s = c.shared.lock().unwrap();
        assert_eq!(s.costs["treeblock 8/132/89"].peak_mb, 400, "its cost under its kind and first block");
        assert!(s.tasks.by_id.is_empty());
    }
}

#[test]
fn a_row_waits_on_no_worker_but_one_measured_faster() {
    let sq = squares_dir();
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    piece(p, sq.path(), "alone", None);
    let (c, url, clock) = coordinator(p);
    let o = Offload::at(url.clone(), c.job_token.clone(), &p.join("job")).clock(clock.clone());
    let tok = c.contact.token.clone();
    let ask = crate::coord::Ask { kind: "native".into(), can: vec![KIND.into()], mem_mb: 4096, ..Default::default() };
    // A worker measured slower than this Mac, asking: never given the row, never waited for.
    let slow = Client::at(vec![url.clone()], tok.clone(), "slow");
    assert!(slow.ask(&ask).unwrap().is_none());
    c.shared.lock().unwrap().tasks.paces.insert(("slow".into(), KIND.into()), 2.0);
    piece(p, sq.path(), "slow", Some(&o));
    assert_eq!(clock.slept().0, 0, "not waited for");
    assert!(slow.ask(&ask).unwrap().is_none(), "given none");
    assert!(same_piece(&p.join("alone"), &p.join("slow")));
    assert!(c.shared.lock().unwrap().tasks.by_id.is_empty());
    // One not measured takes the row in the moment it's given (once an hour) and holds it: the
    // row made here at its turn, at once (no wait on it); the coordinator keeps it for that
    // worker to finish, to measure it.
    let new = Client::at(vec![url.clone()], tok.clone(), "new");
    assert!(new.ask(&ask).unwrap().is_none());
    let w = worker(&clock, &url, &tok, "new", sq.path().into(), p.join("new"), true);
    piece(p, sq.path(), "held", Some(&o));
    assert!(w.lock().unwrap().is_some(), "it took the row");
    assert_eq!(clock.slept(), (1, std::time::Duration::from_secs(1)), "a moment for it to take it, then raced at once");
    assert!(same_piece(&p.join("alone"), &p.join("held")));
    let s = c.shared.lock().unwrap();
    assert!(s.tasks.by_id.values().all(|t| t.measuring.is_some()), "kept only to measure it");
    assert_eq!(s.tasks.by_id.len(), 1);
}

/// Room-making while tree cover runs (docs/building.md §2, store::cachefile): a run reads this Mac's
/// whole copies of the canopy squares (the units') where it has them, held, so room-making can't
/// delete one under it; the NAS's where it hasn't (only the rows it needs, read first: task #606).
/// With room-making trimming the cache as fast as it can the whole time, the run makes what it makes
/// left alone, and the same from the NAS's alone.
#[test]
fn room_making_mid_run_never_reads_a_square_as_none_there() {
    // (No network: what isn't on the scratch NAS fails, never downloads.)
    buildstore::fetch::go_offline();
    let d = squares_dir();
    let o = tempfile::tempdir().unwrap();
    const COV: &str = r#"{"shapes": [[[[5.66, 48.2], [7.2, 48.2], [7.2, 48.91], [5.66, 48.91]]]]}"#;
    std::fs::write(o.path().join("cov.json"), COV).unwrap();
    // The NAS's store has the squares, and the agent's cache (`chm10/`) copies of them.
    let (cache, sources) = (o.path().join("cache"), o.path().join("nas/sources"));
    let (chm, store) = (cache.join("chm10"), sources.join("canopy"));
    std::fs::create_dir_all(&store).unwrap();
    std::fs::create_dir_all(&chm).unwrap();
    for k in ["cover5m", "p95"] {
        std::fs::copy(d.path().join(chm_name(50, 0, k)), store.join(chm_name(50, 0, k))).unwrap();
        std::fs::copy(d.path().join(chm_name(50, 0, k)), chm.join(chm_name(50, 0, k))).unwrap();
    }
    let run = |out: &str| {
        let a = Run {
            tile: (3, 4, 2),
            coverage: o.path().join("cov.json"),
            chm: chm.clone(),
            chm_store: store.clone(),
            sources: sources_file(o.path()),
            leaf: d.path().into(),
            out: o.path().join(out),
            keep: None,
        };
        let n = z3(&a).unwrap();
        // (This process's holds let go, as the job's end would.)
        for k in ["cover5m", "p95"] {
            store::cachefile::release(&chm.join(chm_name(50, 0, k)));
        }
        n
    };
    let calm = run("calm");
    assert!(calm.iter().sum::<usize>() > 0);
    let stop = std::sync::atomic::AtomicBool::new(false);
    let (busy, trims) = std::thread::scope(|s| {
        let room = s.spawn(|| {
            let mut n = 0;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                crate::agent::room::make_room(
                    &cache,
                    &store::nasfs::NasPath::local_for_test(&sources),
                    u64::MAX,
                    0,
                    &Default::default(),
                )
                .unwrap();
                n += 1;
            }
            n
        });
        // (A failure lets room-making go too: the scope waits for it.)
        let busy = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run("busy")));
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        (busy.unwrap_or_else(|e| std::panic::resume_unwind(e)), room.join().unwrap())
    });
    assert!(trims > 0);
    assert_eq!(busy, calm);
    for l in LAYERS {
        let f = format!("trees-{l}.tiles");
        assert_eq!(
            std::fs::read(o.path().join("busy").join(&f)).unwrap(),
            std::fs::read(o.path().join("calm").join(&f)).unwrap(),
            "{l}: the same with room-making under way"
        );
    }
    // No copies here: the NAS's rows, the same.
    std::fs::remove_dir_all(&chm).unwrap();
    assert_eq!(run("nas"), calm);
    for l in LAYERS {
        let f = format!("trees-{l}.tiles");
        assert_eq!(
            std::fs::read(o.path().join("nas").join(&f)).unwrap(),
            std::fs::read(o.path().join("calm").join(&f)).unwrap(),
            "{l}: the same from the NAS's rows"
        );
    }
    assert!(!o.path().join("nas").join(ROWS_DIR).exists(), "the rows read are removed");
}
