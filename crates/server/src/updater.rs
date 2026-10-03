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

/// Whether the map has had a request in the last `secs` seconds.
pub fn in_use(secs: u64) -> bool {
    let t = LAST_REQUEST.load(Ordering::Relaxed);
    t != 0 && now().saturating_sub(t) < secs
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
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
        touch();
        Arc::new(Updater { home: home.to_path_buf(), running, pending: AtomicBool::new(false) })
    }

    pub fn running(&self) -> Option<&str> {
        self.running.as_deref()
    }

    /// Copy a newer published app here and switch `current` to it. True when it switched.
    fn check(&self, data: &Data) -> Result<bool> {
        let Some(running) = self.running.as_deref() else { return Ok(false) };
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
    /// sort by age.
    fn prune(&self, current: &str, running: &str) {
        let dir = self.home.join("app");
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
            .spawn(move || loop {
                if !me.pending.load(Ordering::Relaxed) {
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
                std::thread::sleep(Duration::from_secs(if me.pending.load(Ordering::Relaxed) { 10 } else { 300 }));
            })
            .ok();
    }
}
