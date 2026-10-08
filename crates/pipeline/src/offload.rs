//! A unit job's tasks (docs/workers.md §3): the part of a unit's tail any worker can run, offered
//! through the build Mac's coordinator (crate::coord) while the job prepares its next units. The
//! steps that read the caches (the canopy files) run here, where the data is; those that read only
//! the unit's folder (`Run::reads`) may run anywhere: the files they read are cloned into the task's
//! own folder (copy-on-write: instant, and the job may go on in the unit's), the coordinator serves
//! them to the worker that takes the task, and takes back what it wrote.
//!
//! Nothing is waited on longer than this Mac would take itself: when the job needs a task, one no
//! one took waits up to half a minute while a worker that could take it is asking for work (a page
//! asks every 15–20 s), and one a worker holds waits only while that worker will be back with it
//! sooner than this Mac's own run of it would end (`Patience`); then it's taken back and run here,
//! or, a worker holding it, run here too (whichever finishes first counts; a worker's result that
//! comes in too is compared). A worker's first results are all
//! checked against this Mac's own run of the same steps, then one in eight (the coordinator says
//! which): the steps are deterministic, so any difference is the worker's fault, and it gets no
//! more work.

use crate::coord::client::Client;
use crate::legacy::Unit;
use crate::unit::Run;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct Offload {
    client: Client,
    owner: u32,
    /// The programs' version (the job's binary): a worker fetches their WebAssembly builds by it.
    version: String,
    /// Where tasks' folders go.
    dir: PathBuf,
}

/// A task offered: its id and folder.
pub struct Offered {
    pub id: u64,
    pub root: PathBuf,
}

/// How long the job gives a task no one took, while a worker that could take it is asking for
/// work, before taking it back (a page with a slot idle asks every 15–20 s).
pub const LEASE_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// How long this Mac's own run of a task is taken to be when it doesn't know (seconds): what a
/// worker holding it may take at most.
pub const UNKNOWN_HERE_S: f64 = 180.0;

/// How long the job waits on a task when it needs it, before running it here.
#[derive(Clone, Copy, Debug)]
pub struct Patience {
    /// This Mac's own run of it (seconds; None: not known, `UNKNOWN_HERE_S`).
    pub here_s: Option<f64>,
}

impl Patience {
    /// Whether to go on waiting on a worker that has held the task `age_s` and says it's `frac`
    /// through (None: not said yet), `waited_s` after the job began waiting on it: while its
    /// projected end (its time so far over how far it is) comes before this Mac's own run, begun
    /// when the waiting began, would end. Not said yet: for the whole of this Mac's time.
    pub fn wait_on(&self, waited_s: f64, age_s: f64, frac: Option<f64>) -> bool {
        let budget = self.here_s.unwrap_or(UNKNOWN_HERE_S);
        if waited_s >= budget {
            return false;
        }
        match frac.filter(|f| *f > 0.0) {
            Some(f) => age_s * (1.0 - f.min(1.0)) / f <= budget - waited_s,
            None => true,
        }
    }
}

/// How a unit's tail went, for its log line.
pub enum Settled {
    /// A worker's result, used.
    Remote(String),
    /// Run here; a worker's result compared with it too (its name, and whether it was the same).
    Here(Option<(String, bool)>),
}

impl Offload {
    /// The coordinator this job may offer tasks through (`SCENIC_COORD`, `SCENIC_COORD_TOKEN`: the
    /// agent's, on the build Mac); tasks' folders under `scratch`.
    pub fn from_env(scratch: &Path) -> Option<Offload> {
        let url = std::env::var("SCENIC_COORD").ok()?;
        let token = std::env::var("SCENIC_COORD_TOKEN").ok()?;
        Some(Offload::at(url, token, scratch))
    }

    /// Offering through the coordinator at `url` with the job's token; tasks' folders under
    /// `scratch` (emptied).
    pub fn at(url: String, token: String, scratch: &Path) -> Offload {
        let owner = std::process::id();
        let version = std::env::current_exe().and_then(std::fs::metadata).map(|m| format!("{:x}-{:x}", m.len(), m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs()))).unwrap_or_default();
        let dir = scratch.join("tasks");
        std::fs::remove_dir_all(&dir).ok();
        Offload { client: Client::at(vec![url], token, &format!("job {owner}")), owner, version, dir }
    }

    /// How many tails may be out at once now: one per worker around that takes them, at most three
    /// (each holds a unit's folder on this disk until it's back); none when no one's there.
    pub fn depth(&self) -> usize {
        self.workers("tail").min(3)
    }

    /// How many workers that take tasks of `kind` are around (none when the coordinator can't say).
    pub fn workers(&self, kind: &str) -> usize {
        let v = self.client.post_json("/task/workers", &serde_json::json!({ "kind": kind })).map(|r| r.1);
        v.ok().and_then(|v| v["workers"].as_u64()).map_or(0, |n| n as usize)
    }

    /// The programs' version a task names (a worker fetches their WebAssembly builds by it).
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
    pub fn offer_spec(&self, kind: &str, spec: serde_json::Value, root: &Path, inputs: BTreeMap<String, u64>, mem_mb: u64) -> Result<Offered> {
        let offer = crate::coord::task::Offer { owner: self.owner, kind: kind.into(), spec, root: root.to_path_buf(), inputs, mem_mb };
        let (_, v) = self.client.post_json("/task/offer", &serde_json::to_value(&offer)?)?;
        Ok(Offered { id: v["id"].as_u64().context("the coordinator gave no task id")?, root: root.to_path_buf() })
    }

    /// Offers `runs` over unit `u`'s folder `dir` (and its roadside buildings `bdir`).
    pub fn offer(&self, u: Unit, dir: &Path, bdir: Option<&Path>, runs: &[Run]) -> Result<Offered> {
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
                    clone(&src, &dst)?;
                    inputs.insert(rel, std::fs::metadata(&dst)?.len());
                }
            }
        }
        // Its memory: its files twice (a worker holds them, and a program reads them in), and room
        // to work as much again, what the steps write meanwhile and a step's own (6/20/22's whole
        // tail: 996 MB of files, 565 MB written, the view step 1 GB), with 300 MB at least. What the
        // unit's task took last time, when a worker measured it, replaces this (the coordinator's).
        let mem_mb = (inputs.values().sum::<u64>() >> 20) * 3 + 300;
        let list: Vec<serde_json::Value> = inputs.iter().map(|(p, n)| serde_json::json!([p, n])).collect();
        let spec = serde_json::json!({ "unit": u.slash(), "version": self.version, "runs": runs, "inputs": list, "places": places() });
        self.offer_spec("tail", spec, &root, inputs, mem_mb)
    }

    /// Settles task `t` of the unit in `dir`; with `wait` None only when a worker finished or failed
    /// it (None otherwise), else after the patience given. `here` runs its steps in `dir`.
    pub fn settle(&self, t: &Offered, dir: &Path, wait: Option<Patience>, here: &mut dyn FnMut() -> Result<()>) -> Result<Option<Settled>> {
        let root = t.root.clone();
        self.settle_with(t, wait, here, &mut |st| take(st, dir), &mut |st, since| same(st, dir, &root, since))
    }

    /// Task `t`'s status, after waiting on it as `p` allows (the build pausing: not at all): a
    /// moment for a worker to take it (`LEASE_WAIT`, while one that could is asking for work), and
    /// then, a worker holding it, while it'll be back before this Mac's own run would end.
    fn wait_on(&self, t: &Offered, mut st: serde_json::Value, p: Patience) -> Result<serde_json::Value> {
        let status = || self.client.post_json(&format!("/task/{}", t.id), &serde_json::json!({})).map(|r| r.1);
        let began = std::time::Instant::now();
        let mut held: Option<(std::time::Instant, String)> = None;
        loop {
            if crate::control::draining() {
                return Ok(st);
            }
            match st["state"].as_str() {
                Some("offered") if held.is_none() && began.elapsed() < LEASE_WAIT && st["takers"].as_u64().unwrap_or(0) > 0 => {}
                Some("leased") => {
                    let since = held.get_or_insert_with(|| (std::time::Instant::now(), st["worker"].as_str().unwrap_or("").to_string())).0;
                    if !p.wait_on(since.elapsed().as_secs_f64(), st["age_s"].as_f64().unwrap_or(0.0), st["frac"].as_f64()) {
                        eprintln!("offload: task {}: {} still has it after {:.0} s, not back before this Mac's run would be: run here too", t.id, st["worker"].as_str().unwrap_or(""), since.elapsed().as_secs_f64());
                        return Ok(st);
                    }
                }
                _ => {
                    if let Some((since, w)) = &held {
                        eprintln!("offload: task {}: waited {:.0} s on {w} ({})", t.id, since.elapsed().as_secs_f64(), st["state"].as_str().unwrap_or("gone"));
                    }
                    return Ok(st);
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
            st = status()?;
        }
    }

    /// Settles task `t`, any kind's: with `wait` None only when a worker finished or failed it
    /// (None otherwise), else after the patience given (`wait_on`). A worker's result unchecked is
    /// taken (`take`, with the task's status: its outputs in `out`); else it's run here (`here`),
    /// and a worker's result that came in too compared with this Mac's run (`same`, with the status
    /// and when that run began). Its folder goes.
    pub fn settle_with(&self, t: &Offered, wait: Option<Patience>, here: &mut dyn FnMut() -> Result<()>, take: &mut dyn FnMut(&serde_json::Value) -> Result<()>, same: &mut dyn FnMut(&serde_json::Value, std::time::SystemTime) -> Result<bool>) -> Result<Option<Settled>> {
        let mut st = self.client.post_json(&format!("/task/{}", t.id), &serde_json::json!({}))?.1;
        if let Some(p) = wait {
            st = self.wait_on(t, st, p)?;
        }
        let state = st["state"].as_str().unwrap_or("gone").to_string();
        let worker = st["worker"].as_str().unwrap_or("").to_string();
        let mut checked = None;
        let how = match state.as_str() {
            "done" if st["check"].as_bool() != Some(true) => {
                take(&st)?;
                Settled::Remote(worker)
            }
            "done" => {
                let since = std::time::SystemTime::now();
                here()?;
                let same = same(&st, since)?;
                checked = Some(same);
                Settled::Here(Some((worker, same)))
            }
            "failed" | "gone" => {
                here()?;
                Settled::Here(None)
            }
            _ if wait.is_none() => return Ok(None),
            _ => {
                // Taken back if no one has it; raced if someone does.
                let withdrawn = self.client.post_json(&format!("/task/{}/withdraw", t.id), &serde_json::json!({}))?.1["withdrawn"].as_bool() == Some(true);
                let since = std::time::SystemTime::now();
                here()?;
                let late = if withdrawn { None } else { self.client.post_json(&format!("/task/{}", t.id), &serde_json::json!({})).ok().map(|r| r.1) };
                match late.filter(|s| s["state"] == "done") {
                    Some(s) => {
                        let same = same(&s, since)?;
                        checked = Some(same);
                        Settled::Here(Some((s["worker"].as_str().unwrap_or("").to_string(), same)))
                    }
                    None => Settled::Here(None),
                }
            }
        };
        self.client.post_json(&format!("/task/{}/close", t.id), &serde_json::json!({ "checked": checked })).ok();
        std::fs::remove_dir_all(&t.root).ok();
        Ok(Some(how))
    }
}

/// Where a task's places lie for a worker: its files in a browser's in-memory folders, what's read
/// where it lies under `/net` (the NAS's data and the DEM servers' files, through the coordinator);
/// a native worker maps `/net/nas/` to its own NAS (run_task).
pub fn places() -> serde_json::Value {
    serde_json::json!({
        "{dir}": "/u", "{cache}": "/cache", "{scache}": "/u/scache", "{buildings}": "/b", "{store}": null,
        "{sources}": "/net/nas/sources", "{moi}": "/net/nas/inputs/moi-dtm", "{chm}": "/net/nas/sources/canopy", "{net}": "/net/web",
    })
}

/// Runs task `lease` here, natively (a worker's agent: the M1's): its files fetched from the
/// coordinator into `dir`, its steps run over them with the programs in `bin`, and the files they
/// changed sent back; what to hand back with its done (outputs, inputs removed, time, peak memory).
/// What a page reads through the coordinator (`/net`), it reads where it lies: the NAS's data in
/// `root`, the DEM servers' files over the network. (Without `root`, a task whose steps read the
/// NAS fails here.)
pub fn run_task(client: &Client, lease: u64, spec: &serde_json::Value, dir: &Path, bin: &Path, root: Option<&Path>) -> Result<serde_json::Value> {
    std::fs::remove_dir_all(dir).ok();
    std::fs::create_dir_all(dir.join("u"))?;
    let inputs: Vec<(String, u64)> = serde_json::from_value(spec["inputs"].clone()).context("the task's inputs")?;
    let runs: Vec<Run> = serde_json::from_value(spec["runs"].clone()).context("the task's steps")?;
    // How far it is, for the status: its files fetched, each of its steps (as far as each says),
    // what they wrote sent back.
    let steps = runs.len() as u64 + 2;
    crate::agent::jobs::stage(0, steps, "steps (fetching its files)");
    let names: Vec<String> = runs.iter().map(|r| r.what.clone()).collect();
    crate::unit::on_stage(Some(Box::new(move |what, frac, _| {
        if let Some(i) = names.iter().position(|n| n == what) {
            crate::agent::jobs::report_f((1 + i) as f64 + frac, steps, &format!("steps ({what})"));
        }
    })));
    // (Each input's hash: what's written back unchanged isn't sent, the unit's folder has it.)
    let mut sent = std::collections::HashMap::new();
    for (p, n) in &inputs {
        let rel = crate::coord::task::safe(p).with_context(|| format!("a task input outside its folder: {p}"))?;
        let b = client.get_bytes(&format!("/work/in/{lease}/{p}"))?;
        anyhow::ensure!(b.len() as u64 == *n, "{p}: {} bytes, not {n}", b.len());
        sent.insert(p.clone(), store::naming::hash16(&b));
        let f = dir.join(rel);
        std::fs::create_dir_all(f.parent().unwrap())?;
        std::fs::write(&f, b)?;
    }
    // (What the steps write is told by its time: after this.)
    std::thread::sleep(std::time::Duration::from_millis(20));
    let started = std::time::SystemTime::now();
    // (The NAS's places, here: as the build Mac has them. A worker without the NAS fails only a
    // task whose steps read it.)
    let uses = |k: &str| runs.iter().any(|r| r.args.iter().chain(r.env.iter().map(|(_, v)| v)).any(|v| v.contains(k)));
    let nas = |p: &str| -> Result<Option<PathBuf>> {
        match (spec["places"][p].as_str().and_then(|v| v.strip_prefix("/net/nas/")), root) {
            (Some(rel), Some(root)) => Ok(Some(root.join(rel))),
            (Some(_), None) if uses(p) => anyhow::bail!("the task reads the NAS, which this worker hasn't"),
            _ => Ok(None),
        }
    };
    let tools = crate::unit::Tools { bin: bin.to_path_buf(), dem: PathBuf::new(), cache: dir.join("cache"), buildings: Some(dir.join("b")), moi_dtm: nas("{moi}")?, sources: nas("{sources}")?, shared: None, chm: nas("{chm}")?, stores_read_only: true, spacing_m: 8, snap: None };
    crate::unit::take_peak();
    let t = std::time::Instant::now();
    crate::unit::run_tail(&runs, &dir.join("u"), &tools)?;
    let (secs, peak_mb) = (t.elapsed().as_secs_f64(), crate::unit::take_peak() >> 20);
    crate::unit::on_stage(None);
    crate::agent::jobs::stage(steps - 1, steps, "steps (sending what they wrote)");
    let had: std::collections::BTreeSet<&str> = inputs.iter().map(|(p, _)| p.as_str()).collect();
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
        outputs.push(crate::coord::task::Output { path: rel, size: b.len() as u64 });
    }
    let removed: Vec<&str> = had.iter().filter(|p| p.starts_with("u/") && !dir.join(p).exists()).copied().collect();
    crate::agent::jobs::report(steps, steps, "steps");
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
                    if name.starts_with(start) && std::fs::metadata(e.path()).is_ok_and(|m| m.is_file()) && !name.ends_with(".tmp") {
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

/// A copy-on-write clone of `src` (APFS), else a copy; a link stays a link (to the same file: the
/// coordinator serves it from there).
fn clone(src: &Path, dst: &Path) -> Result<()> {
    #[cfg(unix)]
    if std::fs::symlink_metadata(src)?.file_type().is_symlink() {
        let to = std::fs::read_link(src)?;
        let to = if to.is_relative() { src.parent().unwrap().join(to) } else { to };
        std::os::unix::fs::symlink(to, dst)?;
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let (s, d) = (std::ffi::CString::new(src.as_os_str().as_bytes())?, std::ffi::CString::new(dst.as_os_str().as_bytes())?);
        if unsafe { libc::clonefile(s.as_ptr(), d.as_ptr(), 0) } == 0 {
            return Ok(());
        }
    }
    store::sys::copy_data(src, dst).with_context(|| format!("copy {} to {}", src.display(), dst.display()))?;
    Ok(())
}

/// A finished task's outputs (`u/<path>`) in the unit's folder.
fn place(dir: &Path, path: &str) -> Option<PathBuf> {
    path.strip_prefix("u/").and_then(crate::coord::task::safe).map(|p| dir.join(p))
}

/// Moves a worker's outputs into the unit's folder, and removes what it removed.
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

/// File `path`'s bytes as compared, what says how long a run took aside: the elevations' stats
/// (`dem-stats.json`'s `seconds`).
fn timeless(path: &str, b: Vec<u8>) -> Vec<u8> {
    if !path.ends_with("dem-stats.json") {
        return b;
    }
    match serde_json::from_slice::<serde_json::Value>(&b) {
        Ok(mut v) => {
            if let Some(o) = v.as_object_mut() {
                o.remove("seconds");
            }
            serde_json::to_vec(&v).unwrap_or(b)
        }
        Err(_) => b,
    }
}

/// Whether a worker's outputs are what this Mac's run (from `since`) left in the unit's folder:
/// each it sent is this Mac's, and each file this Mac's run wrote that it didn't send is as the
/// task sent it (`sent`, the task's inputs: a worker sends only what it changed).
fn same(st: &serde_json::Value, dir: &Path, sent: &Path, since: std::time::SystemTime) -> Result<bool> {
    let out = PathBuf::from(st["out"].as_str().context("no outputs' folder")?);
    let mut theirs = std::collections::HashSet::new();
    for o in st["outputs"].as_array().into_iter().flatten() {
        let path = o["path"].as_str().unwrap_or("");
        let Some(mine) = place(dir, path) else { return Ok(false) };
        let read = |p: &Path| std::fs::read(p).ok().map(|b| timeless(path, b));
        if read(&out.join(path)) != read(&mine) {
            eprintln!("offload: {path} differs from this Mac's");
            return Ok(false);
        }
        theirs.insert(path.to_string());
    }
    for (rel, f) in files_under(dir, "u")? {
        if rel == "u/steps.log" || theirs.contains(&rel) || std::fs::metadata(&f)?.modified()? < since {
            continue;
        }
        if std::fs::read(&f).ok() != std::fs::read(sent.join(&rel)).ok() {
            eprintln!("offload: {rel}: this Mac's run changed it, the worker's didn't");
            return Ok(false);
        }
    }
    Ok(st["removed"].as_array().into_iter().flatten().all(|r| r.as_str().and_then(|p| place(dir, p)).is_none_or(|p| !p.exists())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_native_worker_runs_a_task_and_the_job_takes_it() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let (c, port) = crate::coord::start_for_test(&d.path().join("coord"), "m4", "");
        // The unit's folder, and a stand-in step: it writes one file, removes another, and writes
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
        let runs = vec![Run { what: "a step".into(), prog: "step".into(), args: vec!["{dir}".into()], env: vec![], reads: vec!["{dir}/in.bin".into(), "{dir}/gone.bin".into(), "{dir}/kept.bin".into()] }];
        let url = format!("http://127.0.0.1:{port}");
        let o = Offload { client: Client::at(vec![url.clone()], c.job_token.clone(), "job"), owner: 1, version: "v".into(), dir: d.path().join("tasks") };
        let u = Unit::parse("6/1/1").unwrap();
        let t = o.offer(u, &dir, None, &runs).unwrap();
        // The M1: asks, runs it natively, hands it back.
        let m1 = Client::at(vec![url], c.contact.token.clone(), "m1");
        let ask = crate::coord::Ask { kind: "native".into(), can: vec!["unit".into(), "tail".into()], mem_mb: 4096, ..Default::default() };
        let g = m1.ask(&ask).unwrap().unwrap();
        let crate::coord::Granted::Task { task, .. } = g.work else { panic!("not a task") };
        let r = run_task(&m1, g.lease, &task, &d.path().join("m1"), &bin, None).unwrap();
        let done = crate::coord::Done { lease: g.lease, outputs: serde_json::from_value(r["outputs"].clone()).unwrap(), removed: serde_json::from_value(r["removed"].clone()).unwrap(), ..Default::default() };
        assert_eq!(done.removed, ["u/gone.bin"]);
        // (What it wrote as it was isn't sent.)
        assert_eq!(done.outputs.iter().map(|o| o.path.as_str()).collect::<Vec<_>>(), ["u/out.bin"]);
        assert_eq!(m1.done(&done).unwrap(), crate::coord::client::Handed::Taken);
        // The job: a first result is checked; this Mac's run agrees, the worker's outputs match.
        let mut ran = false;
        let mut here = || {
            ran = true;
            crate::unit::run_tail(&runs, &dir, &crate::unit::Tools { bin: bin.clone(), dem: PathBuf::new(), cache: d.path().join("cache"), buildings: None, moi_dtm: None, sources: None, shared: None, chm: None, stores_read_only: false, spacing_m: 8, snap: None })
        };
        let s = o.settle(&t, &dir, None, &mut here).unwrap().unwrap();
        assert!(ran && matches!(s, Settled::Here(Some((ref w, true))) if w == "m1"));
        assert_eq!(std::fs::read(dir.join("out.bin")).unwrap(), b"inmore\n");
        assert_eq!(std::fs::read(dir.join("kept.bin")).unwrap(), b"as it was");
        assert!(!dir.join("gone.bin").exists());
        assert!(!t.root.exists(), "the task's folder goes");
    }

    #[test]
    fn a_worker_is_waited_on_while_it_beats_this_macs_own_run() {
        let p = Patience { here_s: Some(100.0) };
        // Nothing said yet: the whole of this Mac's time.
        assert!(p.wait_on(0.0, 5.0, None) && p.wait_on(99.0, 120.0, None) && !p.wait_on(100.0, 120.0, None));
        // Half through after 40 s: 40 s more, within the 100.
        assert!(p.wait_on(10.0, 40.0, Some(0.5)));
        // A tenth through after 40 s: 360 s more, not.
        assert!(!p.wait_on(10.0, 40.0, Some(0.1)));
        // Waited 80 s: only 20 s left, and it needs 40.
        assert!(!p.wait_on(80.0, 40.0, Some(0.5)));
        // Not known here: three minutes.
        assert!(Patience { here_s: None }.wait_on(170.0, 1.0, None) && !Patience { here_s: None }.wait_on(UNKNOWN_HERE_S, 1.0, None));
    }

    /// A coordinator, a job's Offload and a unit's folder with a stand-in step (it writes one file),
    /// in `d`: (the coordinator, its address, the job's Offload, the folder, the step's programs,
    /// its runs).
    fn setup(d: &Path) -> (crate::coord::Coordinator, String, Offload, PathBuf, PathBuf, Vec<Run>) {
        use std::os::unix::fs::PermissionsExt;
        let (c, port) = crate::coord::start_for_test(&d.join("coord"), "m4", "");
        let dir = d.join("unit");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("in.bin"), b"in").unwrap();
        let bin = d.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("step"), "#!/bin/sh\ncat \"$1/in.bin\" > \"$1/out.bin\"\n").unwrap();
        std::fs::set_permissions(bin.join("step"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let runs = vec![Run { what: "a step".into(), prog: "step".into(), args: vec!["{dir}".into()], env: vec![], reads: vec!["{dir}/in.bin".into()] }];
        let url = format!("http://127.0.0.1:{port}");
        let o = Offload { client: Client::at(vec![url.clone()], c.job_token.clone(), "job"), owner: 1, version: "v".into(), dir: d.join("tasks") };
        (c, url, o, dir, bin, runs)
    }

    #[test]
    fn a_single_units_tail_waits_for_a_worker_asking_for_work() {
        let d = tempfile::tempdir().unwrap();
        let (c, url, o, dir, bin, runs) = setup(d.path());
        let ask = crate::coord::Ask { kind: "native".into(), can: vec!["tail".into()], mem_mb: 4096, ..Default::default() };
        let here = |dir: &Path| crate::unit::run_tail(&runs, dir, &crate::unit::Tools { bin: bin.clone(), dem: PathBuf::new(), cache: d.path().join("cache"), buildings: None, moi_dtm: None, sources: None, shared: None, chm: None, stores_read_only: false, spacing_m: 8, snap: None });
        let p = Patience { here_s: Some(30.0) };
        // No one asking but a page sparing too little for it (300 MB at least): taken back at once,
        // run here.
        let small = Client::at(vec![url.clone()], c.contact.token.clone(), "phone");
        assert!(small.ask(&crate::coord::Ask { kind: "web".into(), can: vec!["tail".into()], mem_mb: 200, ..Default::default() }).unwrap().is_none());
        let t = o.offer(Unit::parse("6/1/1").unwrap(), &dir, None, &runs).unwrap();
        assert_eq!(o.client.post_json(&format!("/task/{}", t.id), &serde_json::json!({})).unwrap().1["takers"], 0);
        let began = std::time::Instant::now();
        let s = o.settle(&t, &dir, Some(p), &mut || here(&dir)).unwrap().unwrap();
        assert!(matches!(s, Settled::Here(None)) && began.elapsed() < std::time::Duration::from_secs(2));
        // A worker asking for work (nothing then), a moment later asking again: it takes the task
        // the job offered as its one unit's, runs it and hands it back, and the job waited for it.
        let m1 = Client::at(vec![url.clone()], c.contact.token.clone(), "m1");
        assert!(m1.ask(&ask).unwrap().is_none());
        let t = o.offer(Unit::parse("6/1/1").unwrap(), &dir, None, &runs).unwrap();
        assert_eq!(o.client.post_json(&format!("/task/{}", t.id), &serde_json::json!({})).unwrap().1["takers"], 1);
        let worker = {
            let (m1, ask, bin, home) = (Client::at(vec![url.clone()], c.contact.token.clone(), "m1"), ask.clone(), bin.clone(), d.path().join("m1"));
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(2));
                let g = m1.ask(&ask).unwrap().unwrap();
                let crate::coord::Granted::Task { task, .. } = g.work else { panic!("not a task") };
                let r = run_task(&m1, g.lease, &task, &home, &bin, None).unwrap();
                let done = crate::coord::Done { lease: g.lease, outputs: serde_json::from_value(r["outputs"].clone()).unwrap(), secs: 1.0, ..Default::default() };
                assert_eq!(m1.done(&done).unwrap(), crate::coord::client::Handed::Taken);
            })
        };
        let began = std::time::Instant::now();
        let s = o.settle(&t, &dir, Some(p), &mut || here(&dir)).unwrap().unwrap();
        worker.join().unwrap();
        // (Its first results are checked: run here too once it was back, and the same.)
        assert!(matches!(s, Settled::Here(Some((ref w, true))) if w == "m1"), "{:?}", matches!(s, Settled::Here(None)));
        assert!(began.elapsed() >= std::time::Duration::from_secs(2));
        assert_eq!(std::fs::read(dir.join("out.bin")).unwrap(), b"in");
    }

    #[test]
    fn a_worker_slower_than_this_mac_is_raced() {
        let d = tempfile::tempdir().unwrap();
        let (c, url, o, dir, _bin, runs) = setup(d.path());
        let ask = crate::coord::Ask { kind: "web".into(), can: vec!["tail".into()], mem_mb: 4096, ..Default::default() };
        let page = Client::at(vec![url], c.contact.token.clone(), "ipad");
        let mut ran = 0;
        // It holds the task and says nothing of how far it is: waited on for this Mac's time (2 s),
        // then raced.
        let t = o.offer(Unit::parse("6/1/1").unwrap(), &dir, None, &runs).unwrap();
        let g = page.ask(&ask).unwrap().unwrap();
        let began = std::time::Instant::now();
        let s = o.settle(&t, &dir, Some(Patience { here_s: Some(2.0) }), &mut || Ok(ran += 1)).unwrap().unwrap();
        assert!(matches!(s, Settled::Here(None)) && ran == 1);
        assert!((2.0..4.0).contains(&began.elapsed().as_secs_f64()));
        // It says it's a hundredth through: it wouldn't be back before this Mac's run (60 s) would
        // end, so it's raced at once.
        let t = o.offer(Unit::parse("6/1/1").unwrap(), &dir, None, &runs).unwrap();
        let g2 = page.ask(&ask).unwrap().unwrap();
        assert_ne!(g.lease, g2.lease);
        std::thread::sleep(std::time::Duration::from_millis(1100));
        page.post_json("/work/beat", &serde_json::json!({ "worker": "ipad", "lease": g2.lease, "frac": 0.01 })).unwrap();
        let began = std::time::Instant::now();
        let s = o.settle(&t, &dir, Some(Patience { here_s: Some(60.0) }), &mut || Ok(ran += 1)).unwrap().unwrap();
        assert!(matches!(s, Settled::Here(None)) && ran == 2 && began.elapsed().as_secs_f64() < 1.5);
    }

    #[test]
    fn a_tasks_files_are_what_its_steps_read() {
        let d = tempfile::tempdir().unwrap();
        let (dir, bdir) = (d.path().join("u"), d.path().join("b"));
        std::fs::create_dir_all(dir.join("scache")).unwrap();
        std::fs::create_dir_all(&bdir).unwrap();
        for f in ["grid.idx", "samples.bin", "terrain.tiles", "scache/view.keys", "scache/view.tmp", "scache/canopy.keys"] {
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
