//! The app on this Mac (docs/macs.md, The updater): every Mac runs it from a local copy of the
//! published app (`<home>/app/<version>/`, `<home>/app/current` pointing at it). When the NAS's
//! `app/current.json` names another version, it's copied here (each file checked by its SHA-256)
//! and `current` is pointed at it; the program that checked restarts into it by its own rule (the
//! agent by its update drain, the map server once the map has been idle a minute). A copy is
//! complete before it's used: files go to `<version>.tmp/`, which is renamed.
//!
//! Every Mac's agent checks (`spawn`, from `scenic agent`); the Mac mini's map server checks too
//! (crates/server/src/updater.rs), so the map updates while the agent there is stopped. One check
//! at a time per Mac: each holds `app/update.lock` (taken without waiting; held, the check is
//! skipped).

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The NAS's record of the published app.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
pub struct Current {
    pub version: String,
    /// Paths relative to the version's folder.
    pub files: Vec<String>,
    /// Each file's SHA-256 (hex), checked after the copy (absent in older manifests).
    #[serde(default)]
    pub sha256: std::collections::BTreeMap<String, String>,
}

/// The file the map server keeps its running version in (`<home>/server-app`): the version
/// pruning keeps for it while it waits for an idle map to restart.
pub const SERVER_APP: &str = "server-app";

/// How long a `<version>.tmp/` folder goes untouched before it's a leftover: no copy in progress
/// (an updater's, or `tools/app/install.sh`'s rsync) pauses that long.
const STALE_COPY: Duration = Duration::from_secs(3600);

/// Whether bytes are a program: a Mach-O binary or a script.
fn executable(b: &[u8]) -> bool {
    b.starts_with(&[0xcf, 0xfa, 0xed, 0xfe]) || b.starts_with(&[0xca, 0xfe, 0xba, 0xbe]) || b.starts_with(b"#!")
}

/// The version the running program was started from (its folder's name under `<home>/app/`; a
/// link through `current` resolves to it), None when it runs from elsewhere (development, tests).
pub fn running_version(home: &Path) -> Option<String> {
    let exe = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok())?;
    let apps = home.join("app").canonicalize().ok()?;
    if !exe.starts_with(&apps) {
        return None;
    }
    exe.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned())
}

/// Copies a newer published app here and points `current` at it: the version it switched to,
/// None when there's nothing to do (or another check holds the lock). `running` is the caller's
/// own version, kept by the pruning.
pub fn check(home: &Path, root: &store::nasfs::NasRoot, running: &str) -> Result<Option<String>> {
    let dir = home.join("app");
    // (A copy that failed, or a version superseded before its copy finished, leaves its `.tmp`
    // folder; checked at every check, NAS or not, not only after a switch.)
    remove_stale_copies(&dir, STALE_COPY);
    let lock = std::fs::File::create(dir.join("update.lock")).context("app/update.lock")?;
    if lock.try_lock().is_err() {
        return Ok(None);
    }
    let Ok(bytes) = root.join("app/current.json").read() else { return Ok(None) };
    let cur: Current = serde_json::from_slice(&bytes).context("app/current.json")?;
    if cur.version.is_empty() || cur.version.contains('/') {
        return Ok(None);
    }
    // Already in place (this check's earlier, or the other program's).
    if std::fs::read_link(dir.join("current")).ok().is_some_and(|l| l.as_os_str() == cur.version.as_str()) {
        return Ok(None);
    }
    let dest = dir.join(&cur.version);
    if !dest.exists() {
        let tmp = dir.join(format!("{}.tmp", cur.version));
        std::fs::remove_dir_all(&tmp).ok();
        for f in &cur.files {
            if f.contains("..") {
                bail!("bad path in app/current.json: {f}");
            }
            let b = root
                .join("app")
                .join(&cur.version)
                .join(f)
                .read()
                .with_context(|| format!("app {}: {f}", cur.version))?;
            if let Some(want) = cur.sha256.get(f) {
                use sha2::Digest;
                let got = format!("{:x}", sha2::Sha256::digest(&b));
                if &got != want {
                    bail!("app {}: {f} doesn't match its SHA-256 (copied {got}, published {want})", cur.version);
                }
            }
            let p = tmp.join(f);
            std::fs::create_dir_all(p.parent().context("a file's folder")?)?;
            std::fs::write(&p, &b)?;
            if executable(&b) {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755))?;
            }
        }
        std::fs::rename(&tmp, &dest)?;
    }
    // Point `current` at it (a new link renamed over the old one).
    let tmp_link = dir.join("current.tmp");
    std::fs::remove_file(&tmp_link).ok();
    std::os::unix::fs::symlink(&cur.version, &tmp_link)?;
    std::fs::rename(&tmp_link, dir.join("current"))?;
    eprintln!("app {} ready", cur.version);
    prune(home, &cur.version, running);
    Ok(Some(cur.version))
}

/// Checks every five minutes on a thread of its own, from a program run from an installed
/// version (else nothing): the agent, which restarts into the new version by its update drain.
pub fn spawn(home: PathBuf, root: Option<store::nasfs::NasRoot>) {
    let Some(running) = running_version(&home) else { return };
    std::thread::Builder::new()
        .name("app-update".into())
        .spawn(move || loop {
            if let Some(r) = root.clone().or_else(|| store::nasfs::NasRoot::find(false)) {
                if let Err(e) = check(&home, &r, &running) {
                    eprintln!("app update: {e:#}");
                }
            }
            std::thread::sleep(Duration::from_secs(300));
        })
        .ok();
}

/// Removes app versions nothing needs: keeps the current one, the caller's, the map server's
/// (`SERVER_APP`), the build agent's (its jobs run programs from its folder; `agent/status.json`
/// says which, and its `job_apps` those of jobs it took over from an older app's agent), and the
/// newest other one (to roll back to). Versions are named from their UTC publish time, so they
/// sort by age. Unfinished copies (`.tmp`) an hour old go too, and the Python environments no
/// version left uses (crate::pyenv::prune).
pub fn prune(home: &Path, current: &str, running: &str) {
    let dir = home.join("app");
    remove_stale_copies(&dir, STALE_COPY);
    let status: Option<serde_json::Value> =
        std::fs::read(home.join("agent/status.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let agent: Option<String> = status.as_ref().and_then(|v| v["app"].as_str().map(str::to_string));
    // (And those its running jobs run from: jobs an older app's agent started, taken over.)
    let job_apps: Vec<String> = status
        .as_ref()
        .and_then(|v| v["job_apps"].as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let server = std::fs::read_to_string(home.join(SERVER_APP)).ok().map(|s| s.trim().to_string());
    let Ok(rd) = std::fs::read_dir(&dir) else { return };
    let mut versions: Vec<String> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.as_bytes().first().is_some_and(u8::is_ascii_digit) && !n.ends_with(".tmp"))
        .collect();
    versions.sort();
    let mut keep: Vec<&str> = vec![current, running];
    keep.extend(agent.as_deref());
    keep.extend(server.as_deref());
    keep.extend(job_apps.iter().map(String::as_str));
    if let Some(v) = versions.iter().rev().find(|v| !keep.contains(&v.as_str())) {
        keep.push(v);
    }
    for v in versions.iter().filter(|v| !keep.contains(&v.as_str())) {
        match std::fs::remove_dir_all(dir.join(v)) {
            Ok(()) => eprintln!("app {v}: removed (replaced)"),
            Err(e) => eprintln!("app {v}: can't remove it: {e}"),
        }
    }
    // (The Python environments the versions left share no longer, a day after their last use.)
    for n in crate::pyenv::prune(home, crate::pyenv::UNUSED_KEEP) {
        eprintln!("python environment {n}: removed (no version uses it)");
    }
}

/// The newest modification time in a folder's tree (the folder itself included).
fn newest_mtime(p: &Path) -> Option<SystemTime> {
    let md = std::fs::symlink_metadata(p).ok()?;
    let mut t = md.modified().ok()?;
    if md.is_dir() {
        for e in std::fs::read_dir(p).ok()?.flatten() {
            if let Some(m) = newest_mtime(&e.path()) {
                t = t.max(m);
            }
        }
    }
    Some(t)
}

/// Removes the half-copied `<version>.tmp/` folders in `<home>/app/` nothing has written to for
/// `age`: an updater's copy that failed or was superseded, or an install that was stopped. A copy
/// under way writes a file at least every few seconds, so it's never one of them.
fn remove_stale_copies(dir: &Path, age: Duration) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !(name.ends_with(".tmp") && name.as_bytes().first().is_some_and(u8::is_ascii_digit)) {
            continue;
        }
        // Folders only (`current.tmp` is a link, and isn't a version's).
        if !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let idle = newest_mtime(&e.path()).and_then(|t| SystemTime::now().duration_since(t).ok());
        if idle.is_some_and(|d| d >= age) {
            match std::fs::remove_dir_all(e.path()) {
                Ok(()) => eprintln!("app {name}: removed (a copy left unfinished)"),
                Err(err) => eprintln!("app {name}: can't remove it: {err}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn age(p: &Path, secs: u64) {
        let t = SystemTime::now() - Duration::from_secs(secs);
        let f = std::fs::File::options().read(true).open(p).unwrap();
        f.set_modified(t).unwrap();
    }

    #[test]
    fn stale_copies_go_and_live_ones_stay() {
        let d = tempfile::tempdir().unwrap();
        let app = d.path();
        // A copy left two hours ago (the M1's app/20261005-1328-ace4faa.tmp).
        let old = app.join("20261005-1328-ace4faa.tmp");
        std::fs::create_dir_all(old.join("web")).unwrap();
        std::fs::write(old.join("server"), b"half").unwrap();
        std::fs::write(old.join("web/index.html"), b"x").unwrap();
        for p in [old.join("server"), old.join("web/index.html"), old.join("web"), old.clone()] {
            age(&p, 7200);
        }
        // A copy under way: an old folder that got a file a moment ago, deep inside.
        let live = app.join("20261008-0100-abc1234.tmp");
        std::fs::create_dir_all(live.join("wasm")).unwrap();
        std::fs::write(live.join("wasm/tile.wasm"), b"x").unwrap();
        age(&live.join("wasm"), 7200);
        age(&live, 7200);
        // An installed version and the `current.tmp` link are no copies.
        let inst = app.join("20261001-0000-0000000");
        std::fs::create_dir_all(&inst).unwrap();
        age(&inst, 7200);
        std::os::unix::fs::symlink("20261001-0000-0000000", app.join("current.tmp")).unwrap();

        remove_stale_copies(app, STALE_COPY);
        assert!(!old.exists());
        assert!(live.join("wasm/tile.wasm").exists());
        assert!(inst.exists());
        assert!(std::fs::symlink_metadata(app.join("current.tmp")).is_ok());
    }

    #[test]
    fn prune_removes_a_stale_copy_and_keeps_the_servers() {
        let d = tempfile::tempdir().unwrap();
        let app = d.path().join("app");
        for v in ["20261001-0000-aaaaaaa", "20261002-0000-bbbbbbb", "20261003-0000-ccccccc", "20261004-0000-eeeeeee"] {
            std::fs::create_dir_all(app.join(v)).unwrap();
        }
        let tmp = app.join("20261002-1200-ddddddd.tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("server"), b"half").unwrap();
        age(&tmp.join("server"), 7200);
        age(&tmp, 7200);
        // The map server still on the first, waiting for an idle map.
        std::fs::write(d.path().join(SERVER_APP), "20261001-0000-aaaaaaa\n").unwrap();
        prune(d.path(), "20261004-0000-eeeeeee", "20261004-0000-eeeeeee");
        assert!(!tmp.exists());
        assert!(app.join("20261001-0000-aaaaaaa").exists(), "the map server's");
        assert!(!app.join("20261002-0000-bbbbbbb").exists());
        assert!(app.join("20261003-0000-ccccccc").exists(), "the one to roll back to");
    }

    #[test]
    fn a_new_version_is_copied_checked_and_made_current() {
        let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::create_dir_all(home.path().join("app/20261001-0000-aaaaaaa")).unwrap();
        std::os::unix::fs::symlink("20261001-0000-aaaaaaa", home.path().join("app/current")).unwrap();
        let v = "20261002-0000-bbbbbbb";
        std::fs::create_dir_all(nas.path().join("app").join(v).join("web")).unwrap();
        std::fs::write(nas.path().join("app").join(v).join("scenic"), b"#!/bin/sh\n").unwrap();
        std::fs::write(nas.path().join("app").join(v).join("web/index.html"), b"x").unwrap();
        use sha2::Digest;
        let sha = |b: &[u8]| format!("{:x}", sha2::Sha256::digest(b));
        let cur = serde_json::json!({"version": v, "files": ["scenic", "web/index.html"],
            "sha256": {"scenic": sha(b"#!/bin/sh\n"), "web/index.html": sha(b"x")}});
        std::fs::write(nas.path().join("app/current.json"), cur.to_string()).unwrap();
        let root = store::nasfs::NasRoot::local_for_test(nas.path());
        // Another check holding the lock: skipped.
        let held = std::fs::File::create(home.path().join("app/update.lock")).unwrap();
        held.lock().unwrap();
        assert_eq!(check(home.path(), &root, "20261001-0000-aaaaaaa").unwrap(), None);
        drop(held);
        assert_eq!(check(home.path(), &root, "20261001-0000-aaaaaaa").unwrap().as_deref(), Some(v));
        assert_eq!(std::fs::read_link(home.path().join("app/current")).unwrap().to_str(), Some(v));
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(home.path().join("app").join(v).join("scenic")).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111);
        // In place: nothing more to do.
        assert_eq!(check(home.path(), &root, "20261001-0000-aaaaaaa").unwrap(), None);
        // A file that doesn't match its SHA-256: refused, `current` left as it was.
        let w = "20261003-0000-ccccccc";
        std::fs::create_dir_all(nas.path().join("app").join(w)).unwrap();
        std::fs::write(nas.path().join("app").join(w).join("scenic"), b"other").unwrap();
        let cur = serde_json::json!({"version": w, "files": ["scenic"], "sha256": {"scenic": sha(b"#!/bin/sh\n")}});
        std::fs::write(nas.path().join("app/current.json"), cur.to_string()).unwrap();
        assert!(check(home.path(), &root, v).is_err());
        assert_eq!(std::fs::read_link(home.path().join("app/current")).unwrap().to_str(), Some(v));
    }
}
