//! The map server's app (docs/macs.md, The updater): it runs from a local copy of the published
//! app (`<home>/app/<version>/`, through `<home>/app/current`, which the launcher runs). Every five
//! minutes it checks for a newer one (pipeline::appupdate, which the agent runs too, one check at a
//! time); once `current` names another version than its own, it exits when the map has been idle
//! a minute, and the launcher starts the new one. Its version is kept in `<home>/server-app`, so
//! pruning keeps it meanwhile.

use crate::data::Data;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub struct Updater {
    home: PathBuf,
    /// The version this process runs (its folder's name), if it runs from `<home>/app/`.
    running: Option<String>,
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

impl Updater {
    pub fn new(home: &Path) -> Arc<Updater> {
        // (Starting isn't a use: until a request comes, the server warms nothing and polls the
        // NAS at the idle rate.)
        Arc::new(Updater { home: home.to_path_buf(), running: pipeline::appupdate::running_version(home) })
    }

    pub fn running(&self) -> Option<&str> {
        self.running.as_deref()
    }

    /// Whether `current` names another version than this process's.
    fn pending(&self, running: &str) -> bool {
        std::fs::read_link(self.home.join("app/current"))
            .ok()
            .and_then(|l| l.file_name().map(|n| n.to_string_lossy().into_owned()))
            .is_some_and(|cur| cur != running)
    }

    /// Check for a new app every five minutes; once one is in place, exit when idle for a minute.
    pub fn spawn(self: &Arc<Self>, data: Arc<Data>) {
        let Some(running) = self.running.clone() else { return };
        if let Err(e) = std::fs::write(self.home.join(pipeline::appupdate::SERVER_APP), format!("{running}\n")) {
            eprintln!("{}: {e}", pipeline::appupdate::SERVER_APP);
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
                        if let Some(root) = data.nas_root() {
                            if let Err(e) = pipeline::appupdate::check(&me.home, &root, &running) {
                                eprintln!("app update: {e:#}");
                            }
                        }
                    }
                    if me.pending(&running) && now().saturating_sub(LAST_REQUEST.load(Ordering::Relaxed)) >= 60 {
                        eprintln!("exiting for the new app");
                        std::process::exit(0);
                    }
                    std::thread::sleep(Duration::from_secs(10));
                }
            })
            .ok();
    }
}
