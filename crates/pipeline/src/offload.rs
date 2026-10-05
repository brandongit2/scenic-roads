//! A unit job's tasks (docs/workers.md §3): the part of a unit's tail any worker can run, offered
//! through the build Mac's coordinator (crate::coord) while the job prepares its next units. The
//! steps that read the caches (the canopy files) run here, where the data is; those that read only
//! the unit's folder (`Run::reads`) may run anywhere: the files they read are cloned into the task's
//! own folder (copy-on-write: instant, and the job may go on in the unit's), the coordinator serves
//! them to the worker that takes the task, and takes back what it wrote.
//!
//! Nothing is waited on that this Mac could do itself: a task no one took is taken back and run
//! here, and one a worker still holds when the job needs it is run here too (whichever finishes
//! first counts; a worker's result that comes in too is compared). A worker's first results are all
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
        let owner = std::process::id();
        let version = std::env::current_exe().and_then(std::fs::metadata).map(|m| format!("{:x}-{:x}", m.len(), m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs()))).unwrap_or_default();
        let dir = scratch.join("tasks");
        std::fs::remove_dir_all(&dir).ok();
        Some(Offload { client: Client::at(vec![url], token, &format!("job {owner}")), owner, version, dir })
    }

    /// How many tails may be out at once now: one per worker around that takes them, at most three
    /// (each holds a unit's folder on this disk until it's back); none when no one's there.
    pub fn depth(&self) -> usize {
        let v = self.client.post_json("/task/workers", &serde_json::json!({ "kind": "tail" })).map(|r| r.1);
        v.ok().and_then(|v| v["workers"].as_u64()).map_or(0, |n| n.min(3) as usize)
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
        // to work; the coordinator raises it to what the unit's task took last time.
        let mem_mb = (inputs.values().sum::<u64>() >> 20) * 2 + 500;
        let list: Vec<serde_json::Value> = inputs.iter().map(|(p, n)| serde_json::json!([p, n])).collect();
        let spec = serde_json::json!({
            "unit": u.slash(), "version": self.version, "runs": runs, "inputs": list,
            "places": { "{dir}": "/u", "{cache}": "/cache", "{scache}": "/u/scache", "{buildings}": "/b", "{store}": null },
        });
        let offer = crate::coord::task::Offer { owner: self.owner, kind: "tail".into(), spec, root: root.clone(), inputs, mem_mb };
        let (_, v) = self.client.post_json("/task/offer", &serde_json::to_value(&offer)?)?;
        Ok(Offered { id: v["id"].as_u64().context("the coordinator gave no task id")?, root })
    }

    /// Settles task `t` of the unit in `dir`; with `wait` false only when a worker finished or failed
    /// it (None otherwise). `here` runs its steps in `dir`.
    pub fn settle(&self, t: &Offered, dir: &Path, wait: bool, here: &mut dyn FnMut() -> Result<()>) -> Result<Option<Settled>> {
        let st = self.client.post_json(&format!("/task/{}", t.id), &serde_json::json!({}))?.1;
        let state = st["state"].as_str().unwrap_or("gone").to_string();
        let worker = st["worker"].as_str().unwrap_or("").to_string();
        let mut checked = None;
        let how = match state.as_str() {
            "done" if st["check"].as_bool() != Some(true) => {
                take(&st, dir)?;
                Settled::Remote(worker)
            }
            "done" => {
                here()?;
                let same = same(&st, dir)?;
                checked = Some(same);
                Settled::Here(Some((worker, same)))
            }
            "failed" | "gone" => {
                here()?;
                Settled::Here(None)
            }
            _ if !wait => return Ok(None),
            _ => {
                // Taken back if no one has it; raced if someone does.
                let withdrawn = self.client.post_json(&format!("/task/{}/withdraw", t.id), &serde_json::json!({}))?.1["withdrawn"].as_bool() == Some(true);
                here()?;
                let late = if withdrawn { None } else { self.client.post_json(&format!("/task/{}", t.id), &serde_json::json!({})).ok().map(|r| r.1) };
                match late.filter(|s| s["state"] == "done") {
                    Some(s) => {
                        let same = same(&s, dir)?;
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

/// Runs task `lease` here, natively (a worker's agent: the M1's): its files fetched from the
/// coordinator into `dir`, its steps run over them with the programs in `bin`, and the files they
/// wrote sent back; what to hand back with its done (outputs, inputs removed, time, peak memory).
pub fn run_task(client: &Client, lease: u64, spec: &serde_json::Value, dir: &Path, bin: &Path) -> Result<serde_json::Value> {
    std::fs::remove_dir_all(dir).ok();
    std::fs::create_dir_all(dir.join("u"))?;
    let inputs: Vec<(String, u64)> = serde_json::from_value(spec["inputs"].clone()).context("the task's inputs")?;
    for (p, n) in &inputs {
        let rel = crate::coord::task::safe(p).with_context(|| format!("a task input outside its folder: {p}"))?;
        let b = client.get_bytes(&format!("/work/in/{lease}/{p}"))?;
        anyhow::ensure!(b.len() as u64 == *n, "{p}: {} bytes, not {n}", b.len());
        let f = dir.join(rel);
        std::fs::create_dir_all(f.parent().unwrap())?;
        std::fs::write(&f, b)?;
    }
    // (What the steps write is told by its time: after this.)
    std::thread::sleep(std::time::Duration::from_millis(20));
    let started = std::time::SystemTime::now();
    let runs: Vec<Run> = serde_json::from_value(spec["runs"].clone()).context("the task's steps")?;
    let tools = crate::unit::Tools { bin: bin.to_path_buf(), dem: PathBuf::new(), cache: dir.join("cache"), buildings: Some(dir.join("b")), moi_dtm: None, sources: None, shared: None, spacing_m: 8, snap: None };
    crate::unit::take_peak();
    let t = std::time::Instant::now();
    crate::unit::run_tail(&runs, &dir.join("u"), &tools)?;
    let (secs, peak_mb) = (t.elapsed().as_secs_f64(), crate::unit::take_peak() >> 20);
    let had: std::collections::BTreeSet<&str> = inputs.iter().map(|(p, _)| p.as_str()).collect();
    let mut outputs = Vec::new();
    for (rel, f) in files_under(&dir.join("u"), "u")? {
        let written = std::fs::metadata(&f)?.modified()? >= started || !had.contains(rel.as_str());
        if !written || rel == "u/steps.log" {
            continue;
        }
        let b = std::fs::read(&f)?;
        client.put_bytes(&format!("/work/out/{lease}/{rel}"), &b)?;
        outputs.push(crate::coord::task::Output { path: rel, size: b.len() as u64 });
    }
    let removed: Vec<&str> = had.iter().filter(|p| p.starts_with("u/") && !dir.join(p).exists()).copied().collect();
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
    std::fs::copy(src, dst).with_context(|| format!("copy {} to {}", src.display(), dst.display()))?;
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
            std::fs::copy(out.join(path), &dst).with_context(|| format!("take {path}"))?;
        }
    }
    for r in st["removed"].as_array().into_iter().flatten() {
        if let Some(p) = r.as_str().and_then(|p| place(dir, p)) {
            std::fs::remove_file(p).ok();
        }
    }
    Ok(())
}

/// Whether a worker's outputs are what this Mac's run left in the unit's folder.
fn same(st: &serde_json::Value, dir: &Path) -> Result<bool> {
    let out = PathBuf::from(st["out"].as_str().context("no outputs' folder")?);
    for o in st["outputs"].as_array().into_iter().flatten() {
        let path = o["path"].as_str().unwrap_or("");
        let Some(mine) = place(dir, path) else { return Ok(false) };
        if std::fs::read(out.join(path)).ok() != std::fs::read(&mine).ok() {
            eprintln!("offload: {path} differs from this Mac's");
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
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let c = crate::coord::Coordinator::start(&d.path().join("coord"), None, port, "m4").unwrap();
        // The unit's folder, and a stand-in step: it writes one file and removes another.
        let dir = d.path().join("unit");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("in.bin"), b"in").unwrap();
        std::fs::write(dir.join("gone.bin"), b"x").unwrap();
        let bin = d.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("step"), "#!/bin/sh\ncat \"$1/in.bin\" > \"$1/out.bin\"; echo more >> \"$1/out.bin\"; rm \"$1/gone.bin\"\n").unwrap();
        std::fs::set_permissions(bin.join("step"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let runs = vec![Run { what: "a step".into(), prog: "step".into(), args: vec!["{dir}".into()], env: vec![], reads: vec!["{dir}/in.bin".into(), "{dir}/gone.bin".into()] }];
        let url = format!("http://127.0.0.1:{port}");
        let o = Offload { client: Client::at(vec![url.clone()], c.job_token.clone(), "job"), owner: 1, version: "v".into(), dir: d.path().join("tasks") };
        let u = Unit::parse("6/1/1").unwrap();
        let t = o.offer(u, &dir, None, &runs).unwrap();
        // The M1: asks, runs it natively, hands it back.
        let m1 = Client::at(vec![url], c.contact.token.clone(), "m1");
        let ask = crate::coord::Ask { kind: "native".into(), can: vec!["unit".into(), "tail".into()], mem_mb: 4096, ..Default::default() };
        let g = m1.ask(&ask).unwrap().unwrap();
        let crate::coord::Granted::Task { task, .. } = g.work else { panic!("not a task") };
        let r = run_task(&m1, g.lease, &task, &d.path().join("m1"), &bin).unwrap();
        let done = crate::coord::Done { lease: g.lease, outputs: serde_json::from_value(r["outputs"].clone()).unwrap(), removed: serde_json::from_value(r["removed"].clone()).unwrap(), ..Default::default() };
        assert_eq!(done.removed, ["u/gone.bin"]);
        assert_eq!(m1.done(&done).unwrap(), crate::coord::client::Handed::Taken);
        // The job: a first result is checked; this Mac's run agrees, the worker's outputs match.
        let mut ran = false;
        let mut here = || {
            ran = true;
            crate::unit::run_tail(&runs, &dir, &crate::unit::Tools { bin: bin.clone(), dem: PathBuf::new(), cache: d.path().join("cache"), buildings: None, moi_dtm: None, sources: None, shared: None, spacing_m: 8, snap: None })
        };
        let s = o.settle(&t, &dir, false, &mut here).unwrap().unwrap();
        assert!(ran && matches!(s, Settled::Here(Some((ref w, true))) if w == "m1"));
        assert_eq!(std::fs::read(dir.join("out.bin")).unwrap(), b"inmore\n");
        assert!(!dir.join("gone.bin").exists());
        assert!(!t.root.exists(), "the task's folder goes");
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
