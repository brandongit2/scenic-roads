//! A `unit` job's tasks run on a worker (docs/workers.md §3): `run_task`, a task's steps over the
//! files fetched from the lead's coordinator (crate::coord); a task's kind by its first step is the
//! plan's (`Plan::task_kind`). What the steps offering tasks name (`Offload`, its waits, a task's places) is the
//! offload crate's (docs/crates.md, task #916 step 6e), re-exported here at its old paths.

pub use ::offload::{places, Offered, Offload, Patience, Settled, WaitNote, Waiting, LEASE_WAIT, WAITING_ENV};

use crate::unit::Run;
use anyhow::{Context, Result};
use coordwire::client::Client;
use std::path::{Path, PathBuf};

/// Runs task `lease` here, natively (a worker's agent): its files fetched from the
/// coordinator into `dir`, its steps run over them with the programs in `bin`, and the files they
/// changed sent back; what to hand back with its done (outputs, inputs removed, time, peak memory).
/// What a task reads through the coordinator (`/net`), it reads here where it lies: the NAS's data in
/// `root`, the DEM servers' files over the network. (Without `root`, a task whose steps read the
/// NAS fails here.)
pub fn run_task(
    client: &Client,
    lease: u64,
    spec: &serde_json::Value,
    dir: &Path,
    bin: &Path,
    root: Option<&store::nasfs::NasRoot>,
) -> Result<serde_json::Value> {
    std::fs::remove_dir_all(dir).ok();
    std::fs::create_dir_all(dir.join("u"))?;
    let inputs: Vec<(String, u64)> = serde_json::from_value(spec["inputs"].clone()).context("the task's inputs")?;
    let runs: Vec<Run> = serde_json::from_value(spec["runs"].clone()).context("the task's steps")?;
    // How far it is, for the status: its files fetched, each of its steps (as far as each says),
    // what they wrote sent back.
    let steps = runs.len() as u64 + 2;
    jobrt::jobs::stage(0, steps, "steps (fetching its files)");
    let names: Vec<String> = runs.iter().map(|r| r.what.clone()).collect();
    crate::unit::on_stage(Some(Box::new(move |what, frac, _| {
        if let Some(i) = names.iter().position(|n| n == what) {
            jobrt::jobs::report_f((1 + i) as f64 + frac, steps, &format!("steps ({what})"));
        }
    })));
    // (Each input's hash: what's written back unchanged isn't sent, the z6 tile's folder has it.)
    let mut sent = std::collections::HashMap::new();
    let fetching = jobrt::timings::phase("fetching its files from the coordinator");
    for (p, n) in &inputs {
        let rel = coordwire::task::safe(p).with_context(|| format!("a task input outside its folder: {p}"))?;
        let t = std::time::Instant::now();
        let b = client.get_bytes(&format!("/work/in/{lease}/{p}"))?;
        let secs = t.elapsed().as_secs_f64();
        eprintln!(
            "fetched {p}: {} in {} ({}/s)",
            jobrt::timings::bytes(b.len() as u64),
            jobrt::timings::secs(secs),
            jobrt::timings::bytes((b.len() as f64 / secs.max(1e-3)) as u64)
        );
        anyhow::ensure!(b.len() as u64 == *n, "{p}: {} bytes, not {n}", b.len());
        sent.insert(p.clone(), store::naming::hash16(&b));
        let f = dir.join(rel);
        std::fs::create_dir_all(f.parent().unwrap())?;
        fetching.count(b.len() as u64, 1);
        std::fs::write(&f, b)?;
    }
    drop(fetching);
    // (What the steps write is told by its time: after this.)
    std::thread::sleep(std::time::Duration::from_millis(20));
    let started = std::time::SystemTime::now();
    // (The NAS's places, here: as the lead has them. A worker without the NAS fails only a
    // task whose steps read it.)
    let uses = |k: &str| runs.iter().any(|r| r.args.iter().chain(r.env.iter().map(|(_, v)| v)).any(|v| v.contains(k)));
    let nas = |p: &str| -> Result<Option<PathBuf>> {
        match (spec["places"][p].as_str().and_then(|v| v.strip_prefix("/net/nas/")), root) {
            // (A step's place on the NAS, as its program's argument: tests/nas_access.rs.)
            (Some(rel), Some(root)) => Ok(Some(root.join(rel).unchecked_path())),
            (Some(_), None) if uses(p) => anyhow::bail!("the task reads the NAS, which this worker hasn't"),
            _ => Ok(None),
        }
    };
    let tools = crate::unit::Tools {
        bin: bin.to_path_buf(),
        cache: dir.join("cache"),
        buildings: Some(dir.join("b")),
        sources: nas("{sources}")?,
        shared: None,
        chm: nas("{chm}")?,
        stores_read_only: true,
        snap: None,
        elevation: None,
        declared: None,
    };
    crate::unit::take_peak();
    let t = std::time::Instant::now();
    crate::unit::run_tail(&runs, &dir.join("u"), &tools)?;
    let (secs, peak_mb) = (t.elapsed().as_secs_f64(), crate::unit::take_peak() >> 20);
    crate::unit::on_stage(None);
    jobrt::jobs::stage(steps - 1, steps, "steps (sending what they wrote)");
    let had: std::collections::BTreeSet<&str> = inputs.iter().map(|(p, _)| p.as_str()).collect();
    let sending = jobrt::timings::phase("sending back what they wrote");
    let mut outputs = Vec::new();
    for (rel, f) in files_under(&dir.join("u"), "u")? {
        let written = std::fs::metadata(&f)?.modified()? >= started || !had.contains(rel.as_str());
        if !written || rel == "u/steps.log" {
            continue;
        }
        let b = std::fs::read(&f)?;
        if sent.get(&rel).is_some_and(|h| *h == store::naming::hash16(&b)) {
            continue;
        }
        client.put_bytes(&format!("/work/out/{lease}/{rel}"), &b)?;
        sending.count(b.len() as u64, 1);
        outputs.push(coordwire::task::Output { path: rel, size: b.len() as u64 });
    }
    let removed: Vec<&str> = had.iter().filter(|p| p.starts_with("u/") && !dir.join(p).exists()).copied().collect();
    jobrt::jobs::report(steps, steps, "steps");
    Ok(serde_json::json!({ "outputs": outputs, "removed": removed, "secs": secs, "peak_mb": peak_mb }))
}

/// Every file under `dir`: (`prefix/<path>`, the file).
fn files_under(dir: &Path, prefix: &str) -> Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)?.flatten() {
        let (name, p) = (e.file_name().to_string_lossy().into_owned(), e.path());
        if e.file_type()?.is_dir() {
            out.extend(files_under(&p, &format!("{prefix}/{name}"))?);
        } else {
            out.push((format!("{prefix}/{name}"), p));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coord::client::LeaseCalls;
    use ::offload::owner;
    use codecs::legacy::TileId;
    use std::sync::{Arc, Mutex};

    #[test]
    fn a_native_worker_runs_a_task_and_the_job_takes_it() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let (c, port) = crate::coord::start_for_test(&d.path().join("coord"), "m4", "");
        // The z6 tile's folder, and a stand-in step: it writes one file, removes another, and writes
        // a third again as it was.
        let dir = d.path().join("unit");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("in.bin"), b"in").unwrap();
        std::fs::write(dir.join("gone.bin"), b"x").unwrap();
        std::fs::write(dir.join("kept.bin"), b"as it was").unwrap();
        let bin = d.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("step"), "#!/bin/sh\ncat \"$1/in.bin\" > \"$1/out.bin\"; echo more >> \"$1/out.bin\"; rm \"$1/gone.bin\"; cp \"$1/kept.bin\" \"$1/k.tmp\"; mv \"$1/k.tmp\" \"$1/kept.bin\"\n").unwrap();
        std::fs::set_permissions(bin.join("step"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let runs = vec![Run {
            what: "a step".into(),
            prog: "step".into(),
            args: vec!["{dir}".into()],
            env: vec![],
            reads: vec!["{dir}/in.bin".into(), "{dir}/gone.bin".into(), "{dir}/kept.bin".into()],
        }];
        let url = format!("http://127.0.0.1:{port}");
        let o = Offload::for_test(
            Client::at(vec![url.clone()], c.job_token.clone(), "job"),
            1,
            "v",
            d.path().join("tasks"),
            LEASE_WAIT,
            Waiting::ByPace,
            store::clock::real(),
        );
        let u = TileId::parse("6/1/1").unwrap();
        let t = o.offer(u, &dir, None, &runs).unwrap();
        // The M1: asks, runs it natively, hands it back.
        let m1 = Client::at(vec![url], c.contact.token.clone(), "m1");
        let ask = crate::coord::Ask {
            kind: "native".into(),
            can: vec!["unit".into(), "tail".into()],
            mem_mb: 4096,
            ..Default::default()
        };
        let g = m1.ask(&ask).unwrap().unwrap();
        let crate::coord::Granted::Task { task, .. } = g.work else { panic!("not a task") };
        let r = run_task(&m1, g.lease, &task, &d.path().join("m1"), &bin, None).unwrap();
        let done = crate::coord::Done {
            lease: g.lease,
            outputs: serde_json::from_value(r["outputs"].clone()).unwrap(),
            removed: serde_json::from_value(r["removed"].clone()).unwrap(),
            ..Default::default()
        };
        assert_eq!(done.removed, ["u/gone.bin"]);
        // (What it wrote as it was isn't sent.)
        assert_eq!(done.outputs.iter().map(|o| o.path.as_str()).collect::<Vec<_>>(), ["u/out.bin"]);
        assert_eq!(m1.done(&done).unwrap(), crate::coord::client::Handed::Taken);
        // The job takes the worker's result: its outputs in the z6 tile's folder, nothing run here.
        let mut ran = false;
        let mut here = || {
            ran = true;
            crate::unit::run_tail(
                &runs,
                &dir,
                &crate::unit::Tools {
                    bin: bin.clone(),
                    cache: d.path().join("cache"),
                    buildings: None,
                    sources: None,
                    shared: None,
                    chm: None,
                    stores_read_only: false,
                    snap: None,
                    elevation: None,
                    declared: None,
                },
            )
        };
        let s = o.settle(&t, &dir, false, Patience { here_s: None }, &mut here).unwrap().unwrap();
        assert!(!ran && matches!(s, Settled::Remote(ref w) if w == "m1"));
        assert_eq!(std::fs::read(dir.join("out.bin")).unwrap(), b"inmore\n");
        assert_eq!(std::fs::read(dir.join("kept.bin")).unwrap(), b"as it was");
        assert!(!dir.join("gone.bin").exists());
        assert!(!t.root.exists(), "the task's folder goes");
    }

    #[test]
    fn a_jobs_tasks_and_its_programs_end_with_its_process_group() {
        // A program the job runs (the tree cover's `trees`) is in the job's process group: its
        // tasks are kept under the same owner.
        let child = store::spawn::output(store::spawn::command("/bin/sh").args(["-c", "ps -o pgid= -p $$"])).unwrap();
        assert_eq!(String::from_utf8_lossy(&child.stdout).trim().parse::<u32>().unwrap(), owner());
        let d = tempfile::tempdir().unwrap();
        let (c, port) = crate::coord::start_for_test(&d.path().join("coord"), "m4", "");
        let url = format!("http://127.0.0.1:{port}");
        let job = Offload::at(url.clone(), c.job_token.clone(), &d.path().join("job"));
        let prog = Offload::at(url, c.job_token.clone(), &d.path().join("prog"));
        assert_eq!((job.owner(), prog.owner()), (owner(), owner()));
        for (o, name) in [(&job, "a"), (&prog, "b")] {
            let root = o.task_root(name);
            std::fs::create_dir_all(root.join("u")).unwrap();
            std::fs::write(root.join("u/x"), b"x").unwrap();
            o.offer_spec("tail", serde_json::json!({ "unit": "6/1/1" }), &root, [("u/x".to_string(), 1)].into(), 100)
                .unwrap();
        }
        assert_eq!(c.shared.lock().unwrap().tasks.by_id.len(), 2);
        // The agent ends a job's tasks by its process group (a job it starts leads one, so its
        // process id, as an older job's tasks were kept under): both end.
        c.close_tasks(owner());
        assert!(c.shared.lock().unwrap().tasks.by_id.is_empty());
    }

    #[test]
    fn a_task_whose_coordinator_is_gone_or_doesnt_know_the_job_is_run_here() {
        // The lead's coordinator stopped (its Mac no longer leads), or one that took the lead back
        // with a new key for its jobs: each task out is settled by running it here, the job going on.
        let d = tempfile::tempdir().unwrap();
        let (c, port) = crate::coord::start_for_test(&d.path().join("coord"), "m4", "");
        let url = format!("http://127.0.0.1:{port}");
        let offer = |o: &Offload, name: &str| {
            let root = o.task_root(name);
            std::fs::create_dir_all(root.join("u")).unwrap();
            std::fs::write(root.join("u/x"), b"x").unwrap();
            o.offer_spec("tail", serde_json::json!({ "unit": "6/1/1" }), &root, [("u/x".to_string(), 1)].into(), 100)
                .unwrap()
        };
        let settle = |o: &Offload, t: &Offered, wait: bool| {
            let mut ran = false;
            let s = o.settle_with(
                t,
                wait,
                Patience { here_s: Some(1.0) },
                &mut || {
                    ran = true;
                    Ok(())
                },
                &mut |_| panic!("no worker's result"),
            );
            (s.unwrap(), ran)
        };
        // A key the coordinator doesn't know (401): run here, waiting or not.
        let job = Offload::at(url.clone(), c.job_token.clone(), &d.path().join("job"));
        let (t1, t2) = (offer(&job, "a"), offer(&job, "b"));
        let stranger = Offload::at(url.clone(), "not the job's key".into(), &d.path().join("other"));
        assert!(matches!(settle(&stranger, &t1, true), (Some(Settled::Here), true)));
        // The coordinator stopped: run here, waiting or not. (No wait: its listener is closed once
        // `stop` returns, and a connection the job's client kept from its offers ends with the
        // coordinator's runtime, unanswered: either way the ask fails, and the task runs here.)
        c.stop();
        drop(c);
        assert!(matches!(settle(&job, &t2, true), (Some(Settled::Here), true)));
        assert!(matches!(settle(&job, &t2, false), (Some(Settled::Here), true)));
    }

    /// A coordinator, a job's Offload and a z6 tile's folder with a stand-in step (it writes one file),
    /// in `d`, both on a virtual clock: (the clock, the coordinator, its address, the job's Offload,
    /// the folder, the step's programs, its runs).
    fn setup(
        d: &Path,
    ) -> (Arc<store::clock::Virtual>, crate::coord::Coordinator, String, Offload, PathBuf, PathBuf, Vec<Run>) {
        use std::os::unix::fs::PermissionsExt;
        let (c, port) = crate::coord::start_for_test(&d.join("coord"), "m4", "");
        let clock = store::clock::Virtual::new();
        c.shared.lock().unwrap().clock = clock.clone();
        let dir = d.join("unit");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("in.bin"), b"in").unwrap();
        let bin = d.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("step"), "#!/bin/sh\ncat \"$1/in.bin\" > \"$1/out.bin\"\n").unwrap();
        std::fs::set_permissions(bin.join("step"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let runs = vec![Run {
            what: "a step".into(),
            prog: "step".into(),
            args: vec!["{dir}".into()],
            env: vec![],
            reads: vec!["{dir}/in.bin".into()],
        }];
        let url = format!("http://127.0.0.1:{port}");
        let o = Offload::for_test(
            Client::at(vec![url.clone()], c.job_token.clone(), "job"),
            1,
            "v",
            d.join("tasks"),
            std::time::Duration::from_secs(3),
            Waiting::ByPace,
            clock.clone(),
        );
        (clock, c, url, o, dir, bin, runs)
    }

    /// A worker on the clock: `after` seconds from now it asks and runs the task it's given, and
    /// `took` seconds later hands it back (how that went, once it has).
    fn worker(
        clock: &Arc<store::clock::Virtual>,
        url: &str,
        token: &str,
        name: &str,
        bin: &Path,
        home: PathBuf,
        after: f64,
        took: f64,
    ) -> Arc<Mutex<Option<crate::coord::client::Handed>>> {
        let (m, bin) = (Client::at(vec![url.to_string()], token.to_string(), name), bin.to_path_buf());
        let handed = Arc::new(Mutex::new(None));
        let (h, c) = (handed.clone(), clock.clone());
        clock.after(std::time::Duration::from_secs_f64(after), move || {
            let ask = crate::coord::Ask {
                kind: "native".into(),
                can: vec!["tail".into()],
                mem_mb: 4096,
                ..Default::default()
            };
            let g = m.ask(&ask).unwrap().expect("given the task");
            let crate::coord::Granted::Task { task, .. } = g.work else { panic!("not a task") };
            let r = run_task(&m, g.lease, &task, &home, &bin, None).unwrap();
            c.after(std::time::Duration::from_secs_f64(took), move || {
                let done = crate::coord::Done {
                    lease: g.lease,
                    outputs: serde_json::from_value(r["outputs"].clone()).unwrap(),
                    secs: took,
                    ..Default::default()
                };
                *h.lock().unwrap() = Some(m.done(&done).unwrap());
            });
        });
        handed
    }

    #[test]
    fn an_asking_worker_is_waited_on_only_once_its_pace_beats_this_mac() {
        // (On a virtual clock, the coordinator's and the job's: the waits are counted in it, the
        // workers act at its moments, and this Mac's own run takes a tenth of a second of it.)
        use std::time::Duration;
        let d = tempfile::tempdir().unwrap();
        let (clock, c, url, o, dir, bin, runs) = setup(d.path());
        // (Its agent told while it waits on a worker: `WAITING_ENV`.)
        let note = d.path().join("waiting");
        let o = o.noting(&note);
        let ask =
            crate::coord::Ask { kind: "native".into(), can: vec!["tail".into()], mem_mb: 4096, ..Default::default() };
        let here = |dir: &Path| {
            clock.advance(Duration::from_millis(100));
            crate::unit::run_tail(
                &runs,
                dir,
                &crate::unit::Tools {
                    bin: bin.clone(),
                    cache: d.path().join("cache"),
                    buildings: None,
                    sources: None,
                    shared: None,
                    chm: None,
                    stores_read_only: false,
                    snap: None,
                    elevation: None,
                    declared: None,
                },
            )
        };
        let p = Patience { here_s: Some(30.0) };
        let status = |t: &Offered| o.client().post_json(&format!("/task/{}", t.id), &serde_json::json!({})).unwrap().1;
        let unit = TileId::parse("6/1/1").unwrap();
        // How long the job waited (slept) on the clock since `from`.
        let waited = |from: (u64, Duration)| clock.slept().1 - from.1;
        // No one asking but one sparing too little for it (300 MB at least): taken back at once.
        let small = Client::at(vec![url.clone()], c.contact.token.clone(), "m9");
        assert!(small.ask(&crate::coord::Ask { mem_mb: 200, ..ask.clone() }).unwrap().is_none());
        let t = o.offer(unit, &dir, None, &runs).unwrap();
        assert_eq!(status(&t)["takers"], serde_json::json!([]));
        let began = clock.slept();
        assert!(matches!(o.settle(&t, &dir, true, p, &mut || here(&dir)).unwrap().unwrap(), Settled::Here));
        assert_eq!(waited(began), Duration::ZERO, "taken back at once");

        // A worker not measured, asking: the job gives it a moment (once this hour), it takes the
        // task a second later, and the job, its pace unknown, runs it at once itself; the
        // coordinator keeps it for the worker to finish (2 s later), and its pace is measured: 2 s
        // against this Mac's tenth of a second.
        let m1 = Client::at(vec![url.clone()], c.contact.token.clone(), "m2");
        assert!(m1.ask(&ask).unwrap().is_none());
        let t = o.offer(unit, &dir, None, &runs).unwrap();
        assert_eq!(status(&t)["takers"], serde_json::json!([{ "worker": "m2", "pace": null, "explore": true }]));
        let w = worker(&clock, &url, &c.contact.token, "m2", &bin, d.path().join("m1"), 1.0, 2.0);
        let began = clock.slept();
        assert!(matches!(o.settle(&t, &dir, true, p, &mut || here(&dir)).unwrap().unwrap(), Settled::Here));
        assert_eq!(waited(began), Duration::from_secs(1), "raced once taken");
        assert!(!t.root.exists());
        assert!(w.lock().unwrap().is_none(), "the worker is still at it");
        clock.advance(Duration::from_secs(10));
        assert_eq!(*w.lock().unwrap(), Some(crate::coord::client::Handed::Taken));
        let pace = c.shared.lock().unwrap().tasks.pace("m2", "tail").unwrap();
        assert!((pace - 20.0).abs() < 1e-9, "2 s against this Mac's 0.1: {pace}");
        assert!(c.shared.lock().unwrap().tasks.by_id.is_empty(), "closed once measured");

        // Measured slow: never waited for, though it asks.
        assert!(m1.ask(&ask).unwrap().is_none());
        let t = o.offer(unit, &dir, None, &runs).unwrap();
        let began = clock.slept();
        assert!(matches!(o.settle(&t, &dir, true, p, &mut || here(&dir)).unwrap().unwrap(), Settled::Here));
        assert_eq!(waited(began), Duration::ZERO, "taken back at once");

        // Another not measured, asking but never taking it: waited for once (the job's 3 s), not
        // again within the hour.
        let m2 = Client::at(vec![url.clone()], c.contact.token.clone(), "m3");
        assert!(m2.ask(&ask).unwrap().is_none());
        for want in [3, 0] {
            let t = o.offer(unit, &dir, None, &runs).unwrap();
            let began = clock.slept();
            assert!(matches!(o.settle(&t, &dir, true, p, &mut || here(&dir)).unwrap().unwrap(), Settled::Here));
            assert_eq!(waited(began), Duration::from_secs(want));
        }

        // Measured fast (a fifth of this Mac's time): waited for, then on, and its result taken.
        {
            let mut s = c.shared.lock().unwrap();
            s.tasks.paces.insert(("m1".into(), "tail".into()), 0.2);
        }
        let fast = Client::at(vec![url.clone()], c.contact.token.clone(), "m1");
        assert!(fast.ask(&ask).unwrap().is_none());
        std::fs::remove_file(dir.join("out.bin")).unwrap();
        let t = o.offer(unit, &dir, None, &runs).unwrap();
        let w = worker(&clock, &url, &c.contact.token, "m1", &bin, d.path().join("fast"), 1.0, 1.0);
        // (Its waiting file, as the agent would read it halfway through the wait.)
        let told = Arc::new(Mutex::new(None));
        let (n, tl) = (note.clone(), told.clone());
        clock.after(Duration::from_millis(1500), move || *tl.lock().unwrap() = Some(WaitNote::read(&n)));
        let began = clock.slept();
        let mut ran = false;
        let s = o.settle(&t, &dir, true, p, &mut || Ok(ran = true)).unwrap().unwrap();
        // While it waited on m1, the file said so (this process, m1, the task); gone once it stopped
        // waiting. (Never written in the waits above: none was on a worker it went on waiting for.)
        let seen = told.lock().unwrap().clone().expect("read during the wait").expect("the file kept");
        assert_eq!((seen.pid, seen.worker.as_str(), seen.task), (std::process::id(), "m1", t.id));
        assert!(!note.exists());
        assert_eq!(*w.lock().unwrap(), Some(crate::coord::client::Handed::Taken));
        assert!(matches!(s, Settled::Remote(ref w) if w == "m1") && !ran);
        assert_eq!(waited(began), Duration::from_secs(2), "a second for it to take it, one for it to finish");
        assert_eq!(std::fs::read(dir.join("out.bin")).unwrap(), b"in");
        // Its pace weighed in: half the last, half this one's (1 s from its lease against the 30 s
        // taken for here).
        let pace = c.shared.lock().unwrap().tasks.pace("m1", "tail").unwrap();
        assert!((pace - (0.5 * 0.2 + 0.5 / 30.0)).abs() < 1e-9, "{pace}");
        assert!(c.tail_takers(), "the forecast's units give it a moment");
    }

    /// Task #903: a z6 tile's tail offered to a member runs to the same folder as one run here, its
    /// parent job paused after its preparing and after a step it ran here, and resumed: the step
    /// kept isn't offered again (its steps aren't idempotent, as the scenic steps over their own
    /// kept results aren't). Its inputs stay readable while the lease is out, the job's own copy of
    /// them gone; once the job has closed the task, its lease is gone (410), not an input missing.
    #[test]
    fn a_tail_offered_after_a_pause_and_resume_runs_to_the_same_folder_as_here() {
        use crate::unit::{run_tail_kept, split_kept, PREPARE};
        use jobrt::resume_log::{Pieces, STOP_AFTER};
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let bin = d.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        // Each step appends a line counting those before: run twice, it leaves another folder.
        std::fs::write(
            bin.join("step"),
            "#!/bin/sh
echo \"$2 $(($(wc -l < \"$1/out.txt\")))\" >> \"$1/out.txt\"\n",
        )
        .unwrap();
        std::fs::set_permissions(bin.join("step"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let tools = crate::unit::Tools {
            bin: bin.clone(),
            cache: d.path().join("cache"),
            buildings: None,
            sources: None,
            shared: None,
            chm: None,
            stores_read_only: false,
            snap: None,
            elevation: None,
            declared: None,
        };
        let runs: Vec<Run> = (0..3)
            .map(|k| Run {
                what: format!("step {k}"),
                prog: "step".into(),
                args: vec!["{dir}".into(), k.to_string()],
                env: Vec::new(),
                reads: vec!["{dir}/out.txt".into()],
            })
            .collect();
        let prepare = |dir: &Path, pieces: &mut Pieces| {
            pieces.folder_piece(PREPARE, &[dir], || {
                std::fs::create_dir_all(dir)?;
                std::fs::write(dir.join("out.txt"), b"")?;
                Ok(Vec::new())
            })
        };
        // Run here, never paused.
        let fresh = d.path().join("fresh");
        let mut none = Pieces::none();
        prepare(&fresh, &mut none).unwrap();
        run_tail_kept(&runs, 0, &fresh, &[&fresh], &tools, &mut none).unwrap();
        // The job: paused after its preparing, then (no one taking its tail) after its first step
        // run here; resumed, the rest offered.
        let (dir, scratch) = (d.path().join("unit"), d.path().join("scratch"));
        let open = || Pieces::open(&scratch, (6, 1, 2), "unit", "a", true);
        let mut p = open();
        STOP_AFTER.set(Some(1));
        assert!(jobrt::control::is_paused(&prepare(&dir, &mut p).unwrap_err()));
        let mut p = open();
        assert!(p.folders_as_kept(&[&dir]));
        prepare(&dir, &mut p).unwrap();
        let (here, anywhere) = split_kept(&runs, &p);
        assert_eq!((here.len(), anywhere.len()), (0, 3));
        STOP_AFTER.set(Some(1));
        assert!(jobrt::control::is_paused(&run_tail_kept(anywhere, 0, &dir, &[&dir], &tools, &mut p).unwrap_err()));
        STOP_AFTER.set(None);
        let mut p = open();
        assert!(p.folders_as_kept(&[&dir]));
        prepare(&dir, &mut p).unwrap();
        let (here, anywhere) = split_kept(&runs, &p);
        assert_eq!((here.len(), anywhere.len()), (1, 2), "the step run before its pause stays here, as kept");
        run_tail_kept(here, 0, &dir, &[&dir], &tools, &mut p).unwrap();
        let (c, port) = crate::coord::start_for_test(&d.path().join("coord"), "m4", "");
        let url = format!("http://127.0.0.1:{port}");
        let o = Offload::at(url.clone(), c.job_token.clone(), &d.path().join("job"));
        let t = o.offer(TileId::parse("6/1/2").unwrap(), &dir, None, anywhere).unwrap();
        // The job's own copy of its inputs gone (it took the task back, a job after it emptied its
        // scratch): the lease's stay.
        std::fs::remove_dir_all(&t.root).unwrap();
        let m4 = Client::at(vec![url], c.contact.token.clone(), "m4");
        let ask =
            crate::coord::Ask { kind: "native".into(), can: vec!["tail".into()], mem_mb: 4096, ..Default::default() };
        let g = m4.ask(&ask).unwrap().unwrap();
        let crate::coord::Granted::Task { task, .. } = g.work else { panic!("not a task") };
        let r = run_task(&m4, g.lease, &task, &d.path().join("m4"), &bin, None).unwrap();
        let done = crate::coord::Done {
            lease: g.lease,
            outputs: serde_json::from_value(r["outputs"].clone()).unwrap(),
            removed: serde_json::from_value(r["removed"].clone()).unwrap(),
            ..Default::default()
        };
        assert_eq!(m4.done(&done).unwrap(), crate::coord::client::Handed::Taken);
        // The worker's result, used: its outputs placed in the folder, as a run here would leave it.
        let s = o
            .settle(&t, &dir, false, Patience { here_s: None }, &mut || crate::unit::run_tail(anywhere, &dir, &tools))
            .unwrap()
            .unwrap();
        assert!(matches!(s, Settled::Remote(ref w) if w == "m4"));
        let read = |d: &Path| std::fs::read_to_string(d.join("out.txt")).unwrap();
        assert_eq!(read(&dir), read(&fresh));
        assert_eq!(read(&dir), "0 0\n1 1\n2 2\n");
        // Closed: its lease is gone, said so to a worker still fetching.
        let e = m4.get_bytes(&format!("/work/in/{}/u/out.txt", g.lease)).unwrap_err();
        assert!(format!("{e:#}").contains("410"), "{e:#}");
        assert!(c.shared.lock().unwrap().tasks.by_id.is_empty());
    }

    /// Task #903: an input that isn't there as the job offers it is refused at the offer (never
    /// offered for a worker to find missing); one that is is served from the coordinator's own copy.
    #[test]
    fn a_task_is_offered_only_with_inputs_it_can_serve() {
        let d = tempfile::tempdir().unwrap();
        let (c, port) = crate::coord::start_for_test(&d.path().join("coord"), "m4", "");
        let o = Offload::at(format!("http://127.0.0.1:{port}"), c.job_token.clone(), &d.path().join("job"));
        let root = o.task_root("a");
        std::fs::create_dir_all(root.join("u")).unwrap();
        std::fs::write(root.join("u/x"), b"x").unwrap();
        let offer = |inputs: &[(&str, u64)]| {
            let inputs = inputs.iter().map(|(p, n)| (p.to_string(), *n)).collect();
            o.offer_spec("tail", serde_json::json!({ "unit": "6/1/1" }), &root, inputs, 100)
        };
        assert!(offer(&[("u/x", 1), ("u/missing", 1)]).is_err());
        assert!(offer(&[("u/x", 2)]).is_err(), "not the size offered");
        assert!(c.shared.lock().unwrap().tasks.by_id.is_empty());
        let t = offer(&[("u/x", 1)]).unwrap();
        let s = c.shared.lock().unwrap();
        let root_there = &s.tasks.by_id[&t.id].root;
        assert_ne!(root_there, &root);
        assert_eq!(std::fs::read(root_there.join("u/x")).unwrap(), b"x");
    }
}
