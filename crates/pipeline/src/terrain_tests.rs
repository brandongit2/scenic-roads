//! terrain_task's tests that start a coordinator (crate::coord): the terrain crate's own tests
//! can't (a family doesn't depend on pipeline; docs/crates.md §6.2), so they're pipeline's, as the
//! write-set tests are (crate::inputs_tests).
#![cfg(test)]

use crate::coord::client::{Client, LeaseCalls};
use crate::terrain_pack::{build_piece_with, mid_logical, piece_levels, read_mid, RawTiles, Sources};
use crate::terrain_task::testkit::north_piece;
use crate::terrain_task::{groups, subtrees_arg, write_hi, Inputs, HI, KIND, MID};
use buildstore::coverage::Coverage;
use buildstore::out::Out;
use offload::Offload;
use std::path::{Path, PathBuf};

/// The z6 tile's run with `offload` into root `name` under `d`: its hi pack's and mid's content
/// names.
fn piece(
    d: &Path,
    name: &str,
    t: (u32, u32),
    cov: &Coverage,
    raw: &RawTiles,
    src: &Sources,
    offload: Option<&Offload>,
) -> (Option<String>, Option<String>) {
    let mut out =
        Out::unbound(&store::nasfs::NasRoot::local_for_test(&d.join(name)), &d.join(format!("{name}-scratch")))
            .unwrap();
    rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap()
        .install(|| build_piece_with(&mut out, raw, t, cov, src, false, &|_, _, _| {}, offload))
        .unwrap();
    (
        out.get(&crate::layers::hi_logical("terrain", t.0, t.1)).map(str::to_string),
        out.get(&mid_logical(t.0, t.1)).map(|c| {
            let (_, m) = read_mid(&out.path(c)).unwrap();
            format!(
                "{:?}",
                m.quads
                    .iter()
                    .map(|(k, v)| (*k, v.iter().map(|f| f.to_bits()).collect::<Vec<_>>()))
                    .collect::<Vec<_>>()
            )
        }),
    )
}

/// A worker in a thread: asks until it's given a task (each 0.2 s, up to 20 s), runs it as the
/// program `terrainsub` does, sends what it wrote, done.
fn worker(url: &str, token: &str, name: &str, home: PathBuf) -> std::thread::JoinHandle<Option<serde_json::Value>> {
    let m = Client::at(vec![url.to_string()], token.to_string(), name);
    std::thread::spawn(move || {
        let ask =
            crate::coord::Ask { kind: "native".into(), can: vec![KIND.into()], mem_mb: 4096, ..Default::default() };
        let mut g = None;
        for _ in 0..100 {
            if let Some(x) = m.ask(&ask).unwrap() {
                g = Some(x);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        let g = g?;
        let crate::coord::Granted::Task { task, .. } = g.work else { panic!("not a task") };
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
            .map(|a| a.as_str().unwrap().replace("{dir}", &u.to_string_lossy()))
            .collect();
        assert_eq!(args[0], "--in");
        let inp = Inputs::open(Path::new(&args[1])).unwrap();
        let (hi, mid) = inp.make().unwrap();
        write_hi(&u.join(HI), &inp.meta.piece, &inp.meta.subtrees, &hi).unwrap();
        crate::terrain_pack::write_mid(&u.join(MID), inp.piece().unwrap(), &mid).unwrap();
        let mut outputs = Vec::new();
        for f in [HI, MID] {
            let bytes = std::fs::read(u.join(f)).unwrap();
            let path = format!("u/{f}");
            m.put_bytes(&format!("/work/out/{}/{path}", g.lease), &bytes).unwrap();
            outputs.push(crate::coord::task::Output { path, size: bytes.len() as u64 });
        }
        let done = crate::coord::Done { lease: g.lease, outputs, peak_mb: 300, secs: 0.5, ..Default::default() };
        assert_eq!(m.done(&done).unwrap(), crate::coord::client::Handed::Taken);
        Some(task)
    })
}

#[test]
fn a_pieces_subtrees_go_to_workers_and_come_back_the_same() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path();
    let (t, cov, raw) = north_piece(p);
    let levels = piece_levels(&cov, &crate::terrain_zoom::test_world(12), t);
    let src = Sources { sets: crate::terrain_zoom::test_world_ref(), ..Default::default() };
    let alone = piece(p, "alone", t, &cov, &raw, &src, None);
    assert!(alone.0.is_some() && alone.1.is_some());
    let (c, port) = crate::coord::start_for_test(&p.join("coord"), "m4", "");
    let url = format!("http://127.0.0.1:{port}");
    let tok = c.contact.token.clone();
    let ask = crate::coord::Ask { kind: "native".into(), can: vec![KIND.into()], mem_mb: 4096, ..Default::default() };
    // No worker around: nothing offered, the same z6 tile.
    let o = Offload::at(url.clone(), c.job_token.clone(), &p.join("job"));
    assert_eq!(piece(p, "none", t, &cov, &raw, &src, Some(&o)), alone);
    assert!(c.shared.lock().unwrap().tasks.by_id.is_empty());

    // A worker measured fast: its group (the last) waited for and taken, the same z6 tile.
    let m1 = Client::at(vec![url.clone()], tok.clone(), "m1");
    assert!(m1.ask(&ask).unwrap().is_none());
    c.shared.lock().unwrap().tasks.paces.insert(("m1".into(), KIND.into()), 0.2);
    // (Waited on until it's back: this is the exchange, not the waiting rule, and the worker is
    // a thread of this test.)
    let patient = Offload::at(url.clone(), c.job_token.clone(), &p.join("job-patient"))
        .waiting(crate::offload::Waiting::UntilDone);
    let w = worker(&url, &tok, "m1", p.join("m1"));
    assert_eq!(piece(p, "taken", t, &cov, &raw, &src, Some(&patient)), alone);
    let task = w.join().unwrap().unwrap();
    let last = groups(&levels).pop().unwrap();
    assert_eq!(task["subtrees"].as_str(), Some(subtrees_arg(&last).as_str()));
    assert_eq!(task["runs"][0]["prog"], "terrainsub");
    {
        let s = c.shared.lock().unwrap();
        assert_eq!(s.costs[&format!("{KIND} {}", task["unit"].as_str().unwrap())].peak_mb, 300);
        assert!(s.tasks.by_id.is_empty());
    }
}
/// The coverage of `out`'s regions, as scenic-build's steps read it (the latest pass's outlines).
fn live_coverage(out: &Out) -> Coverage {
    let root = out.root();
    let a = crate::inputs::regions::read::source(root, &out.manifest).unwrap();
    crate::inputs::regions::read::accepted_coverage(root, &a, None).unwrap()
}

/// By hand, on the build's own data (docs/workers.md §3): z6 tiles `TT_PIECES` ("6/x/y,…") made
/// again in a scratch root over the NAS (`TT_ROOT`: its terrain hi packs' and mids' folders its
/// own, the rest read only; `TT_SCRATCH`, `TT_RAW` the raw tiles' cache), expecting the same as
/// the manifest has them (hi pack and mid), some of each z6 tile's subtrees made by another worker
/// through a coordinator listening on a port of its own: a thread running each task as `scenic run-task`
/// does (the programs in `TT_BIN`). Its results are taken (the worker's pace set so: this is
/// the exchange, not the waiting rule). Each group's worker, time and peak memory said.
#[test]
#[ignore]
fn live_pieces_with_a_worker() {
    let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} isn't set"));
    let (root, scratch) = (PathBuf::from(env("TT_ROOT")), PathBuf::from(env("TT_SCRATCH")));
    let mut out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &scratch.join("job")).unwrap();
    let cov = live_coverage(&out);
    let raw = RawTiles::with_store(
        &PathBuf::from(env("TT_RAW")),
        &store::nasfs::NasPath::local_for_test(&root.join("sources/aws-terrarium")),
        &crate::terrain_base::terrarium_template(&store::nasfs::NasRoot::local_for_test(&root), &out.manifest).unwrap(),
    );
    let src = Sources { sets: crate::terrain_zoom::test_world_ref(), ..Default::default() };
    let (c, port) = crate::coord::start_for_test(&scratch.join("coord"), "m4", "");
    let url = format!("http://127.0.0.1:{port}");
    let off = Offload::at(url.clone(), c.job_token.clone(), &scratch.join("offload"))
        .waiting(crate::offload::Waiting::UntilDone);
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let w = {
        let (m, bin, dir, stop) = (
            Client::at(vec![url.clone()], c.contact.token.clone(), "m1"),
            PathBuf::from(env("TT_BIN")),
            scratch.join("native"),
            stop.clone(),
        );
        std::thread::spawn(move || {
            let ask = crate::coord::Ask {
                kind: "native".into(),
                can: vec![KIND.into()],
                mem_mb: 8192,
                cores: 8,
                ..Default::default()
            };
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let Some(g) = m.ask(&ask).unwrap() else {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    continue;
                };
                let crate::coord::Granted::Task { task, .. } = g.work else { panic!("not a task") };
                let r = crate::offload::run_task(&m, g.lease, &task, &dir, &bin, None).unwrap();
                let done = crate::coord::Done {
                    lease: g.lease,
                    outputs: serde_json::from_value(r["outputs"].clone()).unwrap(),
                    secs: r["secs"].as_f64().unwrap(),
                    peak_mb: r["peak_mb"].as_u64().unwrap(),
                    ..Default::default()
                };
                m.done(&done).unwrap();
            }
        })
    };
    // (The worker: waited for, up to ten minutes.)
    let began = std::time::Instant::now();
    let who = loop {
        let found = c
            .shared
            .lock()
            .unwrap()
            .workers
            .iter()
            .find(|(_, w)| w.can.iter().any(|k| k == KIND))
            .map(|(n, _)| n.clone());
        if let Some(n) = found {
            break n;
        }
        assert!(began.elapsed().as_secs() < 600, "no worker came");
        std::thread::sleep(std::time::Duration::from_millis(500));
    };
    eprintln!("worker: {who}");
    let pieces: Vec<(u32, u32)> = env("TT_PIECES")
        .split(',')
        .map(|p| codecs::legacy::TileId::parse(p).filter(|u| u.z == 6).map(|u| (u.x, u.y)).unwrap())
        .collect();
    for &t in &pieces {
        let (hi_l, mid_l) = (crate::layers::hi_logical("terrain", t.0, t.1), mid_logical(t.0, t.1));
        let (hi_was, mid_was) = (out.get(&hi_l).map(str::to_string), out.get(&mid_l).map(str::to_string));
        assert!(hi_was.is_some() && mid_was.is_some(), "6/{}/{}: no hi pack or mid in the manifest", t.0, t.1);
        c.shared.lock().unwrap().tasks.paces.insert((who.clone(), KIND.into()), 0.2);
        let t0 = std::time::Instant::now();
        out.remove(&mid_l);
        crate::terrain_pack::build_piece_with(&mut out, &raw, t, &cov, &src, true, &|_, _, _| {}, Some(&off)).unwrap();
        assert_eq!(out.get(&hi_l).map(str::to_string), hi_was, "6/{}/{}'s hi pack", t.0, t.1);
        assert_eq!(out.get(&mid_l).map(str::to_string), mid_was, "6/{}/{}'s mid", t.0, t.1);
        let s = c.shared.lock().unwrap();
        let costs: Vec<String> = s
            .costs
            .iter()
            .filter(|(k, _)| k.starts_with(KIND))
            .map(|(k, v)| format!("{k}: {} MB", v.peak_mb))
            .collect();
        println!(
            "6/{}/{}: the same hi pack ({}) and mid ({}) in {:.1} s; {who}: done {}; costs {costs:?}",
            t.0,
            t.1,
            hi_was.unwrap(),
            mid_was.unwrap(),
            t0.elapsed().as_secs_f64(),
            s.workers[&who].done
        );
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    w.join().unwrap();
}
