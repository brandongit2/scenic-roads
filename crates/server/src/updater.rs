//! The app on this Mac (docs/plan.md §4). The server runs from a local copy of the published app
//! (`<home>/app/<version>/`, with `<home>/app/current` pointing at it, which the launcher runs).
//! When the NAS's `app/current.json` names another version, it's copied here and `current` is
//! pointed at it; the server then exits once the map has been idle a minute, and the launcher starts
//! the new one. A copy is complete before it's used: files go to `<version>.tmp/`, which is renamed.

use crate::data::Data;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

/// Whether bytes are a program: a Mach-O binary or a script.
fn executable(b: &[u8]) -> bool {
    b.starts_with(&[0xcf, 0xfa, 0xed, 0xfe]) || b.starts_with(&[0xca, 0xfe, 0xba, 0xbe]) || b.starts_with(b"#!")
}

pub struct Updater {
    home: PathBuf,
    /// The version this process runs (its folder's name), if it runs from `<home>/app/`.
    running: Option<String>,
    pending: AtomicBool,
}

/// Seconds since the epoch of the last request (set by the router's middleware).
pub static LAST_REQUEST: AtomicU64 = AtomicU64::new(0);

/// Record a request (the app restarts for an update only when it has been idle a while).
pub fn touch() {
    LAST_REQUEST.store(now(), Ordering::Relaxed);
}

/// Whether the map has had a request in the last `secs` seconds (never, before the first).
pub fn in_use(secs: u64) -> bool {
    let t = LAST_REQUEST.load(Ordering::Relaxed);
    t != 0 && now().saturating_sub(t) < secs
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// How long a `<version>.tmp/` folder goes untouched before it's a leftover: no copy in progress
/// (this updater's, or `tools/app/install.sh`'s rsync) pauses that long.
const STALE_COPY: Duration = Duration::from_secs(3600);

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

impl Updater {
    pub fn new(home: &Path) -> Arc<Updater> {
        // Running from <home>/app/<version>/server? (Through the `current` link, the executable's
        // path resolves to the version folder.)
        let exe = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok());
        let apps = home.join("app").canonicalize().ok();
        let running = match (exe, apps) {
            (Some(e), Some(a)) if e.starts_with(&a) => e.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()),
            _ => None,
        };
        // (Starting isn't a use: until a request comes, the server warms nothing and polls the
        // NAS at the idle rate.)
        Arc::new(Updater { home: home.to_path_buf(), running, pending: AtomicBool::new(false) })
    }

    pub fn running(&self) -> Option<&str> {
        self.running.as_deref()
    }

    /// Copy a newer published app here and switch `current` to it. True when it switched.
    fn check(&self, data: &Data) -> Result<bool> {
        let Some(running) = self.running.as_deref() else { return Ok(false) };
        // (A copy that failed, or a version superseded before its copy finished, leaves its
        // `.tmp` folder; checked at every check, NAS or not, not only after a switch.)
        remove_stale_copies(&self.home.join("app"), STALE_COPY);
        let (Some(root), Some(pool)) = (data.nas_root(), data.pool()) else { return Ok(false) };
        let cur_path = root.join("app/current.json");
        let bytes = match pool.read_all(&cur_path) {
            Ok(b) => b,
            Err(_) => return Ok(false),
        };
        let cur: Current = serde_json::from_slice(&bytes).context("app/current.json")?;
        if cur.version == running || cur.version.is_empty() || cur.version.contains('/') {
            return Ok(false);
        }
        // Already in place, waiting for the map to be idle to restart into it.
        if std::fs::read_link(self.home.join("app/current")).ok().is_some_and(|l| l.as_os_str() == cur.version.as_str()) {
            return Ok(false);
        }
        let dest = self.home.join("app").join(&cur.version);
        if !dest.exists() {
            let tmp = self.home.join("app").join(format!("{}.tmp", cur.version));
            std::fs::remove_dir_all(&tmp).ok();
            for f in &cur.files {
                if f.contains("..") {
                    bail!("bad path in app/current.json: {f}");
                }
                // In pieces through the pool: one slow whole-file read would trip the breaker.
                let src = crate::views::RemoteFile::new(root.join("app").join(&cur.version).join(f), pool.clone());
                let b = src.read_all()?;
                if let Some(want) = cur.sha256.get(f) {
                    use sha2::Digest;
                    let got = format!("{:x}", sha2::Sha256::digest(&b));
                    if &got != want {
                        bail!("app {}: {f} doesn't match its SHA-256 (copied {got}, published {want})", cur.version);
                    }
                }
                let p = tmp.join(f);
                std::fs::create_dir_all(p.parent().unwrap())?;
                std::fs::write(&p, &b)?;
                if executable(&b) {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755))?;
                }
            }
            std::fs::rename(&tmp, &dest)?;
        }
        // Point `current` at it (a new link renamed over the old one).
        let link = self.home.join("app/current");
        let tmp_link = self.home.join("app/current.tmp");
        std::fs::remove_file(&tmp_link).ok();
        std::os::unix::fs::symlink(&cur.version, &tmp_link)?;
        std::fs::rename(&tmp_link, &link)?;
        eprintln!("app {} ready; restarting when the map is idle", cur.version);
        self.prune(&cur.version, running);
        Ok(true)
    }

    /// Removes app versions nothing needs: keeps the current one, this server's, the build
    /// agent's (its jobs run programs from its folder; `agent/status.json` says which), and the
    /// newest other one (to roll back to). Versions are named from their UTC publish time, so they
    /// sort by age. Unfinished copies (`.tmp`) an hour old go too.
    fn prune(&self, current: &str, running: &str) {
        let dir = self.home.join("app");
        remove_stale_copies(&dir, STALE_COPY);
        let agent: Option<String> = std::fs::read(self.home.join("agent/status.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| v["app"].as_str().map(str::to_string));
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
        if let Some(v) = versions.iter().rev().find(|v| !keep.contains(&v.as_str())) {
            keep.push(v);
        }
        for v in versions.iter().filter(|v| !keep.contains(&v.as_str())) {
            match std::fs::remove_dir_all(dir.join(v)) {
                Ok(()) => eprintln!("app {v}: removed (replaced)"),
                Err(e) => eprintln!("app {v}: can't remove it: {e}"),
            }
        }
    }

    /// Check for a new app every five minutes; once one is in place, exit when idle for a minute.
    pub fn spawn(self: &Arc<Self>, data: Arc<Data>) {
        if self.running.is_none() {
            return;
        }
        let me = self.clone();
        std::thread::Builder::new()
            .name("updater".into())
            .spawn(move || {
                // Checked every five minutes, waiting for an idle map or not: a version published
                // while another waits takes its place.
                let mut last: Option<std::time::Instant> = None;
                loop {
                    if last.is_none_or(|t| t.elapsed() >= Duration::from_secs(300)) {
                        last = Some(std::time::Instant::now());
                        match me.check(&data) {
                            Ok(true) => me.pending.store(true, Ordering::Relaxed),
                            Ok(false) => {}
                            Err(e) => eprintln!("app update: {e:#}"),
                        }
                    }
                    if me.pending.load(Ordering::Relaxed) && now().saturating_sub(LAST_REQUEST.load(Ordering::Relaxed)) >= 60 {
                        eprintln!("exiting for the new app");
                        std::process::exit(0);
                    }
                    std::thread::sleep(Duration::from_secs(10));
                }
            })
            .ok();
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
    fn prune_removes_a_stale_copy() {
        let d = tempfile::tempdir().unwrap();
        let app = d.path().join("app");
        for v in ["20261001-0000-aaaaaaa", "20261002-0000-bbbbbbb", "20261003-0000-ccccccc"] {
            std::fs::create_dir_all(app.join(v)).unwrap();
        }
        let tmp = app.join("20261002-1200-ddddddd.tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("server"), b"half").unwrap();
        age(&tmp.join("server"), 7200);
        age(&tmp, 7200);
        let u = Updater { home: d.path().to_path_buf(), running: Some("20261003-0000-ccccccc".into()), pending: AtomicBool::new(false) };
        u.prune("20261003-0000-ccccccc", "20261003-0000-ccccccc");
        assert!(!tmp.exists());
        assert!(!app.join("20261001-0000-aaaaaaa").exists());
        assert!(app.join("20261002-0000-bbbbbbb").exists());
    }
}
