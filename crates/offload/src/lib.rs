//! A `unit` job's tasks (docs/workers.md §3): the part of a z6 tile's tail any worker can run, offered
//! through the lead's coordinator (agentcore::coord) while the job prepares its next z6 tiles. The
//! steps that read the caches (the canopy files) run here, where the data is; those that read only
//! the z6 tile's folder (`Run::reads`) may run anywhere: the files they read are cloned into the task's
//! own folder (copy-on-write: instant, and the job may go on in the z6 tile's), the coordinator clones
//! them again into its own (agentcore::coord::task::Tasks::offer: they stay readable until the task
//! ends there, whatever the job does with its folders meanwhile), serves them to the worker that
//! takes the task, and takes back what it wrote. A task that ends while its worker still fetches
//! (the job ran it itself) answers it 410: that lease is gone.
//!
//! Nothing waits on a worker slower than this Mac: each worker's pace at each kind of task is
//! measured (the coordinator's: its time over this Mac's for the same task). When the job needs a
//! task, one no one took waits up to half a minute only while a worker that could take it is
//! asking for work and is measured faster than this Mac (or isn't yet,
//! once an hour); one a worker holds waits only while its pace says it'll be back sooner than this
//! Mac's own run of it would end (`Patience`). Then it's taken back and run here, or, a worker
//! holding it, run here too (whichever finishes first counts; one not measured is kept by the
//! coordinator to finish, to measure it).
//!
//! This crate (docs/crates.md, task #916 step 6e) is what the steps offering tasks name (terrain,
//! trees, 3D buildings, a z6 tile's tail): `Offload` and its waits, a task's steps (`Run`) and
//! places. Running a task on a worker (`run_task`, over the unit's steps) and telling its kind by
//! its first step (`task_kind`) stay pipeline's offload, which re-exports these at their old paths.

use anyhow::{Context, Result};
use codecs::legacy::TileId;
use coordwire::client::Client;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One program a task runs (docs/workers.md §3): its name (the build's `bin/<prog>`), its arguments and environment, in which `{dir}` is the z6 tile's
/// folder, `{cache}` the canopy cache, `{scache}` the folder's scenic cache, `{buildings}` its
/// roadside buildings, `{store}` the NAS's canopy store to download into, and what's read where it
/// lies: `{sources}` the NAS's sources, `{chm}` its canopy squares, `{net}`
/// the DEM servers' files (`places`; an environment variable whose value names a
/// place the worker doesn't have is left out).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Run {
    pub what: String,
    pub prog: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// The files it reads of the z6 tile's own (`{dir}/<name>`, `{buildings}/<name>`; a trailing `*`
    /// for any name so begun), earlier steps' outputs too: a task sends those there when it
    /// starts, so it may run on any worker. None: it stays here (roads::unit::keep_here).
    #[serde(default)]
    pub reads: Vec<String>,
}

pub struct Offload {
    client: Client,
    owner: u32,
    /// The programs' version (the job's binary), which a task's spec names.
    version: String,
    /// Where tasks' folders go.
    dir: PathBuf,
    /// How long a task no one took waits for a worker to take it (`LEASE_WAIT`).
    lease_wait: std::time::Duration,
    /// How long a job waits on a worker holding a task (`Waiting`).
    waiting: Waiting,
    /// The time its waits are measured and slept in (a test's virtual one: `clock`).
    clock: std::sync::Arc<dyn store::clock::Clock>,
    /// The file it tells its agent it waits on a worker in (`WAITING_ENV`), while it does.
    note: Option<PathBuf>,
}

/// The environment variable naming the file a job keeps while it waits on a worker holding one of
/// its tasks, and nothing else (`Patience::wait_on`'s wait): the agent (docs/pool.md §7.7) then
/// counts the job's memory as it holds it now, its cores idle, and may start a second job beside it.
pub const WAITING_ENV: &str = "SCENIC_WAITING";

/// What a job's waiting file says (`WAITING_ENV`): its process (the agent takes the file as stale
/// once that process is gone or isn't its job's), the worker and the task, and since when.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WaitNote {
    pub pid: u32,
    pub worker: String,
    pub task: u64,
    pub since: u64,
}

/// A job's waiting file while it's kept: written as the job begins waiting on a worker, removed as
/// it stops (whatever way it stops: dropped).
pub struct WaitFile {
    path: PathBuf,
}

impl WaitNote {
    /// Writes `self` to `path` (whole: codecs::whole), kept until the returned value drops.
    pub fn keep(&self, path: &Path) -> Option<WaitFile> {
        store::whole::write(path, &serde_json::to_vec(self).ok()?).ok()?;
        Some(WaitFile { path: path.to_path_buf() })
    }

    /// The note in `path`, when there's one that reads.
    pub fn read(path: &Path) -> Option<WaitNote> {
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }
}

impl Drop for WaitFile {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).ok();
    }
}

/// How long a job waits on a worker holding one of its tasks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Waiting {
    /// While the worker's pace says it'll be back before this Mac's own run would end
    /// (`Patience::wait_on`): every job.
    ByPace,
    /// Until the worker hands it back, or its lease lapses: tests of the exchange itself, whose
    /// worker is a thread of the test (the waiting rule has tests of its own).
    UntilDone,
}

/// A task offered: its id and folder.
pub struct Offered {
    pub id: u64,
    pub root: PathBuf,
}

/// How long the job gives a task no one took, while a worker that could take it is asking for
/// work, before taking it back.
pub const LEASE_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// How long this Mac's own run of a task is taken to be when it doesn't know (seconds).
pub const UNKNOWN_HERE_S: f64 = 180.0;

/// What the job knows of a task when it needs it: this Mac's own run of it (seconds; None: not
/// known, `UNKNOWN_HERE_S`). A worker's pace (the coordinator's measure: its time over this Mac's
/// for the same task) says whether waiting on it pays.
#[derive(Clone, Copy, Debug)]
pub struct Patience {
    pub here_s: Option<f64>,
}

impl Patience {
    /// This Mac's own run, as known or taken to be.
    pub fn budget(&self) -> f64 {
        self.here_s.unwrap_or(UNKNOWN_HERE_S)
    }

    /// Whether to go on waiting on a worker at `pace` that has held the task `age_s`, `waited_s`
    /// after the job began waiting on it: while its time (at its pace, with the margin) would end
    /// before this Mac's own run, begun when the waiting began, would. Not measured: not at all.
    pub fn wait_on(&self, waited_s: f64, age_s: f64, pace: Option<f64>) -> bool {
        let budget = self.budget();
        match pace {
            Some(p) if waited_s < budget => p * coordwire::task::MARGIN * budget - age_s <= budget - waited_s,
            _ => false,
        }
    }
}

/// How a z6 tile's tail went, for its log line.
pub enum Settled {
    /// A worker's result, used.
    Remote(String),
    /// Run here.
    Here,
}

impl Settled {
    /// As its log line says it: "by <worker>", or "here".
    pub fn how(&self) -> String {
        match self {
            Settled::Remote(w) => format!("by {w}"),
            Settled::Here => "here".into(),
        }
    }
}

impl Offload {
    /// The coordinator this job may offer tasks through (`SCENIC_COORD`, `SCENIC_COORD_TOKEN`: the
    /// agent's, on the lead); tasks' folders under `scratch`.
    pub fn from_env(scratch: &Path) -> Option<Offload> {
        let url = std::env::var("SCENIC_COORD").ok()?;
        let token = std::env::var("SCENIC_COORD_TOKEN").ok()?;
        Some(Offload::at(url, token, scratch))
    }

    /// Offering through the coordinator at `url` with the job's token; tasks' folders under
    /// `scratch` (emptied).
    pub fn at(url: String, token: String, scratch: &Path) -> Offload {
        let owner = owner();
        let version = std::env::current_exe()
            .and_then(std::fs::metadata)
            .map(|m| {
                format!(
                    "{:x}-{:x}",
                    m.len(),
                    m.modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_secs())
                )
            })
            .unwrap_or_default();
        let dir = scratch.join("tasks");
        std::fs::remove_dir_all(&dir).ok();
        Offload {
            client: Client::at(vec![url], token, &format!("job {owner}")),
            owner,
            version,
            dir,
            lease_wait: LEASE_WAIT,
            waiting: Waiting::ByPace,
            clock: store::clock::real(),
            note: std::env::var_os(WAITING_ENV).map(PathBuf::from),
        }
    }

    /// The same, waiting on a worker holding a task as `w` says.
    pub fn waiting(mut self, w: Waiting) -> Offload {
        self.waiting = w;
        self
    }

    /// The same, its waits measured and slept in `clock` (a test's virtual clock).
    pub fn clock(mut self, clock: std::sync::Arc<dyn store::clock::Clock>) -> Offload {
        self.clock = clock;
        self
    }

    /// The same, telling its agent it waits on a worker in `path` (`WAITING_ENV`; a test's).
    pub fn noting(mut self, path: &Path) -> Offload {
        self.note = Some(path.to_path_buf());
        self
    }

    /// The job its tasks are kept under (`Offer::owner`).
    pub fn owner(&self) -> u32 {
        self.owner
    }

    /// How many tails may be out at once now: one per worker around that takes them, at most three
    /// (each holds a z6 tile's folder on this disk until it's back); none when no one's there.
    pub fn depth(&self) -> usize {
        self.workers("tail").min(3)
    }

    /// How many workers that take tasks of `kind` are around (none when the coordinator can't say).
    pub fn workers(&self, kind: &str) -> usize {
        let v = self.client.post_json("/task/workers", &serde_json::json!({ "kind": kind })).map(|r| r.1);
        v.ok().and_then(|v| v["workers"].as_u64()).map_or(0, |n| n as usize)
    }

    /// The programs' version a task names.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The folder for task `name`'s files (its `u/` what a worker is sent), emptied.
    pub fn task_root(&self, name: &str) -> PathBuf {
        let root = self.dir.join(name);
        std::fs::remove_dir_all(&root).ok();
        root
    }

    /// Offers a task of `kind` (`spec` what a worker is given, `inputs` its files in `root`, its
    /// predicted memory `mem_mb`: the coordinator's measure of its last run replaces it).
    pub fn offer_spec(
        &self,
        kind: &str,
        spec: serde_json::Value,
        root: &Path,
        inputs: BTreeMap<String, u64>,
        mem_mb: u64,
    ) -> Result<Offered> {
        self.offer_spec_ahead(kind, spec, root, inputs, mem_mb, None)
    }

    /// `offer_spec`, saying how much of the job's own work comes before it needs the task (`ahead`,
    /// in tasks' worth of this Mac's time at its kind: agentcore::coord::task::Task::ahead), when its
    /// agent tells it to offer ahead (jobrt::jobs::ahead_on); else as `offer_spec`.
    pub fn offer_spec_ahead(
        &self,
        kind: &str,
        spec: serde_json::Value,
        root: &Path,
        inputs: BTreeMap<String, u64>,
        mem_mb: u64,
        ahead: Option<f64>,
    ) -> Result<Offered> {
        let ahead = ahead.filter(|a| a.is_finite() && *a > 0.0 && jobrt::jobs::ahead_on());
        let offer = coordwire::task::Offer {
            owner: self.owner,
            kind: kind.into(),
            spec,
            root: root.to_path_buf(),
            inputs,
            mem_mb,
            ahead,
        };
        let (_, v) = self.client.post_json("/task/offer", &serde_json::to_value(&offer)?)?;
        Ok(Offered { id: v["id"].as_u64().context("the coordinator gave no task id")?, root: root.to_path_buf() })
    }

    /// Offers `runs` over z6 tile `u`'s folder `dir` (and its roadside buildings `bdir`).
    pub fn offer(&self, u: TileId, dir: &Path, bdir: Option<&Path>, runs: &[Run]) -> Result<Offered> {
        let root = self.dir.join(u.dash());
        std::fs::remove_dir_all(&root).ok();
        let mut inputs: BTreeMap<String, u64> = BTreeMap::new();
        for r in runs {
            for pat in &r.reads {
                for (rel, src) in matching(pat, dir, bdir)? {
                    if inputs.contains_key(&rel) {
                        continue;
                    }
                    let dst = root.join(&rel);
                    std::fs::create_dir_all(dst.parent().unwrap())?;
                    coordwire::task::clone(&src, &dst)
                        .with_context(|| format!("clone {} to {}", src.display(), dst.display()))?;
                    inputs.insert(rel, std::fs::metadata(&dst)?.len());
                }
            }
        }
        // Its memory: its files twice (a worker holds them, and a program reads them in), and room
        // to work as much again, what the steps write meanwhile and a step's own (6/20/22's whole
        // tail: 996 MB of files, 565 MB written, the view step 1 GB), with 300 MB at least. What the
        // z6 tile's task took last time, when a worker measured it, replaces this (the coordinator's).
        let mem_mb = (inputs.values().sum::<u64>() >> 20) * 3 + 300;
        let list: Vec<serde_json::Value> = inputs.iter().map(|(p, n)| serde_json::json!([p, n])).collect();
        // (The spec's `unit` is the task's z6 tile: the key of every task spec, read by workers: kept.)
        let spec = serde_json::json!({ "unit": u.slash(), "version": self.version, "runs": runs, "inputs": list, "places": places() });
        self.offer_spec("tail", spec, &root, inputs, mem_mb)
    }

    /// Settles task `t` of the z6 tile in `dir`; with `wait` false only when a worker finished or
    /// failed it (None otherwise). `here` runs its steps in `dir`.
    pub fn settle(
        &self,
        t: &Offered,
        dir: &Path,
        wait: bool,
        p: Patience,
        here: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Option<Settled>> {
        use jobrt::timings::phase;
        self.settle_with(t, wait, p, here, &mut |st| {
            let _p = phase("taking in a worker's results");
            take(st, dir)
        })
    }

    /// Task `t`'s status, after waiting on it as `p` allows (the build pausing: not at all): a
    /// moment for a worker to take it (`LEASE_WAIT`), while one that could is asking for work whose
    /// pace beats this Mac's, or whose pace isn't measured and wasn't waited for in the last hour (it
    /// gets measured: the coordinator keeps the task for it to finish when the job runs it itself);
    /// then, a worker holding it, while its pace says it'll be back before this Mac's own run would
    /// end.
    /// Task `t`'s status as the coordinator says it; `gone` when it can't say (it stopped, its Mac
    /// no longer leading: docs/pool.md §7.6; it crashed; or it no longer knows this job's key, a
    /// lead taken back with a new one): the job runs the task itself.
    fn status(&self, t: &Offered) -> serde_json::Value {
        match self.client.post_json(&format!("/task/{}", t.id), &serde_json::json!({})) {
            Ok((200, v)) => v,
            Ok((code, _)) => {
                eprintln!("offload: task {}: the coordinator answered {code}: run here", t.id);
                serde_json::json!({ "state": "gone" })
            }
            Err(e) => {
                eprintln!("offload: task {}: the coordinator doesn't answer ({e:#}): run here", t.id);
                serde_json::json!({ "state": "gone" })
            }
        }
    }

    fn wait_on(&self, t: &Offered, mut st: serde_json::Value, p: Patience) -> Result<serde_json::Value> {
        let _p = jobrt::timings::phase("waiting on other workers");
        let _o = store::waits::other();
        let status = || -> Result<serde_json::Value> { Ok(self.status(t)) };
        let clock = &*self.clock;
        let since = |t: std::time::Instant| clock.now().saturating_duration_since(t);
        let began = clock.now();
        let mut held: Option<(std::time::Instant, String)> = None;
        // (Its agent told while it waits on a worker holding the task, nothing else to do: the
        // file goes as the wait ends, whichever way.)
        let mut _told: Option<WaitFile> = None;
        // (Whether to give a worker a moment to take it: decided on the first status.)
        let mut lease_wait = None;
        loop {
            jobrt::control::reached();
            if jobrt::control::draining() {
                return Ok(st);
            }
            match st["state"].as_str() {
                Some("offered") if held.is_none() && since(began) < self.lease_wait => {
                    let wait = *lease_wait.get_or_insert_with(|| {
                        let takers = st["takers"].as_array().cloned().unwrap_or_default();
                        let fast = takers.iter().any(|w| w["pace"].as_f64().is_some_and(coordwire::task::beats));
                        // (Else one not measured, once an hour: told, so the hour starts now.)
                        fast || (takers.iter().any(|w| w["explore"] == true)
                            && self
                                .client
                                .post_json(&format!("/task/{}/explore", t.id), &serde_json::json!({}))
                                .is_ok())
                    });
                    if !wait {
                        return Ok(st);
                    }
                }
                Some("leased") => {
                    let from =
                        held.get_or_insert_with(|| (clock.now(), st["worker"].as_str().unwrap_or("").to_string())).0;
                    if self.waiting == Waiting::ByPace
                        && !p.wait_on(
                            since(from).as_secs_f64(),
                            st["age_s"].as_f64().unwrap_or(0.0),
                            st["pace"].as_f64(),
                        )
                    {
                        let pace = st["pace"]
                            .as_f64()
                            .map_or("not measured".to_string(), |p| format!("{p:.2}× this Mac's time"));
                        eprintln!("offload: task {}: {} has it (its pace {pace}), not back before this Mac's run would be: run here too", t.id, st["worker"].as_str().unwrap_or(""));
                        return Ok(st);
                    }
                    if _told.is_none() {
                        if let Some(path) = &self.note {
                            let since = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map_or(0, |d| d.as_secs());
                            _told = WaitNote {
                                pid: std::process::id(),
                                worker: held.as_ref().map(|h| h.1.clone()).unwrap_or_default(),
                                task: t.id,
                                since,
                            }
                            .keep(path);
                        }
                    }
                }
                _ => {
                    if let Some((from, w)) = &held {
                        eprintln!(
                            "offload: task {}: waited {:.0} s on {w} ({})",
                            t.id,
                            since(*from).as_secs_f64(),
                            st["state"].as_str().unwrap_or("gone")
                        );
                    }
                    return Ok(st);
                }
            }
            clock.sleep(std::time::Duration::from_secs(1));
            st = status()?;
        }
    }

    /// Settles task `t`, any kind's: with `wait` false only when a worker finished or failed it
    /// (None otherwise), else after the patience `p` gives (`wait_on`). A worker's result is taken
    /// (`take`, with the task's status: its outputs in `out`); else it's run here (`here`). Its
    /// folder goes.
    pub fn settle_with(
        &self,
        t: &Offered,
        wait: bool,
        p: Patience,
        here: &mut dyn FnMut() -> Result<()>,
        take: &mut dyn FnMut(&serde_json::Value) -> Result<()>,
    ) -> Result<Option<Settled>> {
        let mut st = self.status(t);
        if wait {
            st = self.wait_on(t, st, p)?;
        }
        // (This Mac's run timed, when it runs it: the worker's pace is measured against it.)
        let mut ran = None;
        let mut here = || -> Result<()> {
            let began = self.clock.now();
            here()?;
            ran = Some(self.clock.now().saturating_duration_since(began).as_secs_f64());
            Ok(())
        };
        let state = st["state"].as_str().unwrap_or("gone").to_string();
        let worker = st["worker"].as_str().unwrap_or("").to_string();
        let how = match state.as_str() {
            "done" => {
                take(&st)?;
                Settled::Remote(worker)
            }
            "failed" | "gone" => {
                here()?;
                Settled::Here
            }
            _ if !wait => return Ok(None),
            _ => {
                // Taken back if no one has it; raced if someone does (this Mac's run counts).
                // (A coordinator that can't answer: run here.)
                self.client.post_json(&format!("/task/{}/withdraw", t.id), &serde_json::json!({})).ok();
                here()?;
                Settled::Here
            }
        };
        // (A run the clock saw take no time, a test's virtual clock, is no measure: the estimate
        // stands.)
        let here_s = ran.filter(|&s| s > 0.0).or(p.here_s);
        let r = self.client.post_json(&format!("/task/{}/close", t.id), &serde_json::json!({ "here_s": here_s }));
        if r.is_ok_and(|r| r.1["measuring"] == true) {
            eprintln!("offload: task {}: kept for its worker to finish, to measure its pace", t.id);
        }
        std::fs::remove_dir_all(&t.root).ok();
        Ok(Some(how))
    }
}

/// Whose tasks a job's are (the coordinator ends them when that job ends: the agent's
/// `close_tasks` with the job's process group): its process group, which a job the agent starts
/// leads (so its process id, as a job's tasks were kept under before), and which a program it runs
/// is in (the tree cover's `trees`, offering its rows of blocks), so theirs end with the job too.
pub fn owner() -> u32 {
    #[cfg(unix)]
    {
        (unsafe { libc::getpgrp() }) as u32
    }
    #[cfg(not(unix))]
    {
        std::process::id()
    }
}

/// Where a task's places lie for a worker: its files under its folder, what's read where it lies
/// under `/net` (the NAS's data and the DEM servers' files); a worker maps `/net/nas/` to its own
/// NAS (run_task).
pub fn places() -> serde_json::Value {
    serde_json::json!({
        "{dir}": "/u", "{cache}": "/cache", "{scache}": "/u/scache", "{buildings}": "/b", "{store}": null,
        "{sources}": "/net/nas/sources", "{chm}": "/net/nas/sources/canopy", "{net}": "/net/web",
    })
}

/// The files pattern `pat` names (`{dir}/<name>`, `{buildings}/<name>`; a trailing `*` matches any
/// name with that start in that folder): (their path in a task's folder, the file here).
fn matching(pat: &str, dir: &Path, bdir: Option<&Path>) -> Result<Vec<(String, PathBuf)>> {
    let (base, prefix, rel) = if let Some(r) = pat.strip_prefix("{dir}/") {
        (dir, "u", r)
    } else if let Some(r) = pat.strip_prefix("{buildings}/") {
        let Some(b) = bdir else { return Ok(Vec::new()) };
        (b, "b", r)
    } else {
        anyhow::bail!("a step reads {pat}: not in its unit's folder");
    };
    let mut out = Vec::new();
    match rel.strip_suffix('*') {
        Some(start) => {
            let (sub, start) = start.rsplit_once('/').map_or(("", start), |(a, b)| (a, b));
            let folder = base.join(sub);
            if let Ok(rd) = std::fs::read_dir(&folder) {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    // (A file, or a link to one: the roadside buildings are links to the NAS's.)
                    if name.starts_with(start)
                        && std::fs::metadata(e.path()).is_ok_and(|m| m.is_file())
                        && !name.ends_with(".tmp")
                    {
                        let r = if sub.is_empty() { name.clone() } else { format!("{sub}/{name}") };
                        out.push((format!("{prefix}/{r}"), folder.join(&name)));
                    }
                }
            }
        }
        None => {
            if base.join(rel).is_file() {
                out.push((format!("{prefix}/{rel}"), base.join(rel)));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// A finished task's outputs (`u/<path>`) in the z6 tile's folder.
fn place(dir: &Path, path: &str) -> Option<PathBuf> {
    path.strip_prefix("u/").and_then(coordwire::task::safe).map(|p| dir.join(p))
}

/// Moves a worker's outputs into the z6 tile's folder, and removes what it removed.
fn take(st: &serde_json::Value, dir: &Path) -> Result<()> {
    let out = PathBuf::from(st["out"].as_str().context("no outputs' folder")?);
    for o in st["outputs"].as_array().into_iter().flatten() {
        let path = o["path"].as_str().unwrap_or("");
        let dst = place(dir, path).with_context(|| format!("a worker wrote {path}, outside the unit's folder"))?;
        std::fs::create_dir_all(dst.parent().unwrap())?;
        // (The coordinator's folder is on the same disk as the job's: a rename; else a copy.)
        if std::fs::rename(out.join(path), &dst).is_err() {
            store::sys::copy_data(out.join(path), &dst).with_context(|| format!("take {path}"))?;
        }
    }
    for r in st["removed"].as_array().into_iter().flatten() {
        if let Some(p) = r.as_str().and_then(|p| place(dir, p)) {
            std::fs::remove_file(p).ok();
        }
    }
    Ok(())
}

/// A test's offload (pipeline's offload tests, through a coordinator of their own): its fields as
/// given, where `at` reads them from its process.
#[cfg(any(test, feature = "testkit"))]
impl Offload {
    #[allow(clippy::too_many_arguments)]
    pub fn for_test(
        client: Client,
        owner: u32,
        version: &str,
        dir: PathBuf,
        lease_wait: std::time::Duration,
        waiting: Waiting,
        clock: std::sync::Arc<dyn store::clock::Clock>,
    ) -> Offload {
        Offload { client, owner, version: version.into(), dir, lease_wait, waiting, clock, note: None }
    }

    /// Its client (a test asks the coordinator of a task's state with it).
    pub fn client(&self) -> &Client {
        &self.client
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_worker_is_waited_on_only_at_a_pace_that_beats_this_mac() {
        let p = Patience { here_s: Some(100.0) };
        // Not measured: raced at once.
        assert!(!p.wait_on(0.0, 0.0, None));
        // Half this Mac's time (62.5 s with the margin): waited on; nine tenths (112.5 s): not.
        assert!(p.wait_on(0.0, 10.0, Some(0.5)) && !p.wait_on(0.0, 10.0, Some(0.9)));
        // Waited 50 s, held 60: 2.5 s more, within the 50 left.
        assert!(p.wait_on(50.0, 60.0, Some(0.5)));
        // Waited 50 s, held only 5 (taken late): 57.5 s more, past the 50 left.
        assert!(!p.wait_on(50.0, 5.0, Some(0.5)));
        // This Mac's time gone: not.
        assert!(!p.wait_on(100.0, 200.0, Some(0.5)));
    }

    #[test]
    fn a_tasks_files_are_what_its_steps_read() {
        let d = tempfile::tempdir().unwrap();
        let (dir, bdir) = (d.path().join("u"), d.path().join("b"));
        std::fs::create_dir_all(dir.join("scache")).unwrap();
        std::fs::create_dir_all(&bdir).unwrap();
        for f in
            ["grid.idx", "samples.bin", "terrain.tiles", "scache/view.keys", "scache/view.tmp", "scache/canopy.keys"]
        {
            std::fs::write(dir.join(f), f).unwrap();
        }
        std::fs::write(bdir.join("8-1-1.f32"), b"x").unwrap();
        let names = |pat: &str| matching(pat, &dir, Some(&bdir)).unwrap().into_iter().map(|x| x.0).collect::<Vec<_>>();
        assert_eq!(names("{dir}/grid.idx"), ["u/grid.idx"]);
        assert_eq!(names("{dir}/scache/view*"), ["u/scache/view.keys"]);
        assert_eq!(names("{buildings}/*"), ["b/8-1-1.f32"]);
        assert!(names("{dir}/missing.bin").is_empty());
        assert!(matching("{cache}/chm10/x", &dir, None).is_err());
        // A clone has the same bytes; outputs placed back only inside the folder.
        let clone = coordwire::task::clone;
        clone(&dir.join("samples.bin"), &d.path().join("c.bin")).unwrap();
        assert_eq!(std::fs::read(d.path().join("c.bin")).unwrap(), b"samples.bin");
        // A link: matched, and linked again to the same file.
        std::os::unix::fs::symlink(dir.join("grid.idx"), bdir.join("8-1-2.f32")).unwrap();
        assert_eq!(names("{buildings}/*"), ["b/8-1-1.f32", "b/8-1-2.f32"]);
        clone(&bdir.join("8-1-2.f32"), &d.path().join("l.f32")).unwrap();
        assert!(std::fs::symlink_metadata(d.path().join("l.f32")).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read(d.path().join("l.f32")).unwrap(), b"grid.idx");
        assert_eq!(place(&dir, "u/near.i8"), Some(dir.join("near.i8")));
        assert!(place(&dir, "u/../x").is_none() && place(&dir, "b/x").is_none());
    }
}
