//! bld::task's test that starts a coordinator (crate::coord: a bldtiles job's z8 tiles to workers
//! and back): the bld crate's own tests can't (a family doesn't depend on pipeline; docs/crates.md
//! §6.2), so it's pipeline's, as terrain_tests and trees_tests are.
#![cfg(test)]

use crate::bld::job;
use crate::bld::task::testkit::{collect, coverage, files, here_s, out, records, Tiles};
use crate::bld::task::{run, take_z8, Offers, KIND, OUTPUTS};
use crate::coord::client::LeaseCalls;
use codecs::legacy::TileId;
use std::path::Path;

/// A worker in this process: asks, fetches the task's files, runs the program's code over them,
/// sends back what it wrote (`spoil`: its tiles spoilt first), done.
fn worker_runs(m1: &crate::coord::client::Client, dir: &Path) -> Option<serde_json::Value> {
    let ask = crate::coord::Ask { kind: "native".into(), can: vec![KIND.into()], mem_mb: 4096, ..Default::default() };
    let g = m1.ask(&ask).unwrap()?;
    let crate::coord::Granted::Task { task, .. } = g.work else { panic!("not a task") };
    assert_eq!(task["runs"][0]["prog"], KIND);
    assert_eq!(task["runs"][0]["args"][0], "{dir}");
    let a = TileId::parse(task["unit"].as_str().unwrap()).unwrap();
    std::fs::remove_dir_all(dir).ok();
    let u = dir.join("u");
    std::fs::create_dir_all(&u).unwrap();
    for i in task["inputs"].as_array().unwrap() {
        let p = i[0].as_str().unwrap();
        std::fs::write(dir.join(p), m1.get_bytes(&format!("/work/in/{}/{p}", g.lease)).unwrap()).unwrap();
    }
    run(&u, a).unwrap();
    let mut outputs = Vec::new();
    for f in OUTPUTS {
        let Ok(b) = std::fs::read(u.join(f)) else { continue };
        m1.put_bytes(&format!("/work/out/{}/u/{f}", g.lease), &b).unwrap();
        outputs.push(crate::coord::task::Output { path: format!("u/{f}"), size: b.len() as u64 });
    }
    let done = crate::coord::Done { lease: g.lease, outputs, peak_mb: 300, secs: 1.0, ..Default::default() };
    assert_eq!(m1.done(&done).unwrap(), crate::coord::client::Handed::Taken);
    Some(task)
}

#[test]
fn z8_tiles_go_to_workers_and_come_back_the_same() {
    let d = tempfile::tempdir().unwrap();
    let t = TileId { z: 6, x: 32, y: 22 };
    let files = files(d.path());
    let cov = coverage();
    let z8s = job::z8_tiles_of(&files);
    let (c, port) =
        crate::coord::start_on_test_port(crate::agent::theplan::the_plan(), &d.path().join("coord"), "m4", "");
    // (The coordinator and the job on a virtual clock: the job's waits are counted in it.)
    let clock = store::clock::Virtual::new();
    c.shared.lock().unwrap().clock = clock.clone();
    let url = format!("http://127.0.0.1:{port}");
    let o = crate::offload::Offload::at(url.clone(), c.job_token.clone(), &d.path().join("job")).clock(clock.clone());
    let scratch = d.path().join("job");
    // No worker around: nothing offered, every z8 tile run here.
    let mut offers = Offers::new(Some(&o), &scratch, t, z8s.len(), &[]);
    offers.top_up(&files, &cov, &z8s, 0).unwrap();
    assert!(out(&offers).is_empty());
    // A worker that takes them asks (nothing yet), and is counted.
    let m1 = crate::coord::client::Client::at(vec![url], c.contact.token.clone(), "m1");
    assert!(worker_runs(&m1, &d.path().join("m1")).is_none());
    offers.top_up(&files, &cov, &z8s, 0).unwrap();
    // The far end's z8 tile offered (one worker: one out), never the one under way.
    assert_eq!(out(&offers).keys().copied().collect::<Vec<_>>(), [1]);
    let task = worker_runs(&m1, &d.path().join("m1")).unwrap();
    assert_eq!(task["unit"], "8/131/88");
    // Before z8 tile 0 runs: the finished one settled (the worker's result taken).
    offers.top_up(&files, &cov, &z8s, 0).unwrap();
    assert!(out(&offers).is_empty());
    let dir = offers.result(&files, &cov, &z8s, 1).unwrap().unwrap();
    let mut got: Tiles = Vec::new();
    let a8 = TileId { z: 8, x: 131, y: 88 };
    take_z8(&dir, a8, &mut collect(&mut got)).unwrap();
    let mut want: Tiles = Vec::new();
    job::z8_tiles(&files, &cov, t, (131, 88), &[], &mut collect(&mut want)).unwrap();
    assert_eq!(got, want);
    assert!(offers.result(&files, &cov, &z8s, 0).unwrap().is_none(), "z8 tile 0 runs here");
    {
        let s = c.shared.lock().unwrap();
        // Its cost learned under its kind and z8 tile.
        assert_eq!(s.costs["bldtile 8/131/88"].peak_mb, 300);
    }
    // Offered again (another job), with the cost learned, and taken again. (Its pace forgotten
    // first: measured slower than this Mac's run on its first z8 tile, it would be given none,
    // coord::task::Tasks::pick.)
    c.shared.lock().unwrap().tasks.paces.clear();
    let mut offers = Offers::new(Some(&o), &scratch, t, z8s.len(), &[]);
    offers.top_up(&files, &cov, &z8s, 0).unwrap();
    assert_eq!(out(&offers).len(), 1);
    assert_eq!(c.shared.lock().unwrap().tasks.by_id.values().next().unwrap().mem_mb, 330);
    worker_runs(&m1, &d.path().join("m1")).unwrap();
    let dir = offers.result(&files, &cov, &z8s, 1).unwrap().unwrap();
    let mut again: Tiles = Vec::new();
    take_z8(&dir, a8, &mut collect(&mut again)).unwrap();
    assert_eq!(again, want, "the worker's");
    // A task no one took (a worker asking, sparing too little for it; the first measured slower
    // than this Mac): taken back and run here at once. (What it would take here: its buildings
    // at the pace of those made here.)
    c.shared.lock().unwrap().tasks.paces.insert(("m1".into(), KIND.into()), 2.0);
    let m2 = crate::coord::client::Client::at(vec![format!("http://127.0.0.1:{port}")], c.contact.token.clone(), "m2");
    m2.ask(&crate::coord::Ask { kind: "native".into(), can: vec![KIND.into()], mem_mb: 10, ..Default::default() })
        .unwrap();
    let mut offers = Offers::new(Some(&o), &scratch, t, z8s.len(), &[]);
    offers.top_up(&files, &cov, &z8s, 0).unwrap();
    assert_eq!(out(&offers).len(), 1);
    assert_eq!(here_s(&offers, 1), None);
    offers.made_here(10, 2.0);
    assert!(records(&offers)[&1] > 0);
    assert_eq!(here_s(&offers, 1), Some(0.2 * records(&offers)[&1] as f64));
    let dir = offers.result(&files, &cov, &z8s, 1).unwrap().unwrap();
    assert_eq!(clock.slept().0, 0, "never waited");
    let mut third: Tiles = Vec::new();
    take_z8(&dir, a8, &mut collect(&mut third)).unwrap();
    assert_eq!(third, want);
    assert!(c.shared.lock().unwrap().tasks.by_id.is_empty(), "closed");
}
