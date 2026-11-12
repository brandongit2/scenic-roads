//! The app's releases from the lead (docs/coordinator.md §10): what every Mac's updater asks of it
//! (jobrt::appupdate's `Source`, on the client), the updater's thread (`spawn`, the agent's;
//! `check`, the map server's too), and a publish's calls (`push`, `rollback`: `scenic releases`,
//! run by tools/app/publish.sh). The lead's side is agentcore's coord::releases.

use crate::client::Client;
use anyhow::{bail, Context, Result};
use jobrt::appupdate::{Live, Manifest, Source};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The lead's live release and the one before (`GET`).
pub const LIVE: &str = "/release/live";
/// A release's archive coming in (`PUT /release/upload/<version>`), then put live from it (`POST`).
pub const UPLOAD: &str = "/release/upload";
pub const INSTALL: &str = "/release/install";
/// The live release and the one before swapped (`POST`).
pub const ROLLBACK: &str = "/release/rollback";

/// What the lead holds: Some (perhaps no release yet), or None from a lead on an app before #1118
/// (its router's bare 404: it keeps no releases).
pub fn live(c: &Client) -> Result<Option<Live>> {
    match c.request("GET", LIVE, None)? {
        (200, b) => Ok(Some(serde_json::from_slice(&b).context("the lead's releases")?)),
        (404, b) if b.is_empty() => Ok(None),
        (code, b) => bail!("the lead's releases: {code} {}", String::from_utf8_lossy(&b)),
    }
}

impl Source for Client {
    fn live(&self) -> Result<Option<Live>> {
        Ok(live(self)?.filter(|l| l.current.is_some()))
    }

    fn fetch(&self, v: &str, to: &Path) -> Result<()> {
        let b = self.get_bytes(&format!("/release/{v}.tgz")).with_context(|| format!("app {v}: its archive"))?;
        std::fs::write(to, b).with_context(|| format!("app {v}: its archive"))
    }
}

/// How a publish or a rollback went: in, or refused as the lead's releases stand (the live one
/// not the one it was asked from: another publish or a rollback went in meanwhile).
#[derive(Debug, PartialEq, Eq)]
pub enum Asked {
    Done(Live),
    Moved(Live),
}

fn asked(path: &str, r: crate::client::Reply) -> Result<Asked> {
    let live =
        |v: &serde_json::Value| -> Result<Live> { serde_json::from_value(v["live"].clone()).context(path.to_string()) };
    match r {
        (200, v) => Ok(Asked::Done(live(&v)?)),
        (409, v) => Ok(Asked::Moved(live(&v)?)),
        (code, v) => bail!("{path}: {code} {v}"),
    }
}

/// Publishes a release to the lead: its archive uploaded, then put live from `base` (the live
/// version it was checked against; None: none live). The lead checks the archive and every file
/// against the manifest (a refusal is an error) and the order (`Asked::Moved`).
pub fn push(c: &Client, archive: &Path, m: &Manifest, base: Option<&str>) -> Result<Asked> {
    let bytes = std::fs::read(archive).with_context(|| archive.display().to_string())?;
    let (code, b) = c.request("PUT", &format!("{UPLOAD}/{}", m.version), Some(&bytes))?;
    anyhow::ensure!(code == 200, "{UPLOAD}: {code} {}", String::from_utf8_lossy(&b));
    let up: serde_json::Value = serde_json::from_slice(&b).context(UPLOAD)?;
    let upload = up["upload"].as_str().context("the lead named no upload")?;
    asked(INSTALL, c.post_json(INSTALL, &serde_json::json!({ "manifest": m, "base": base, "upload": upload }))?)
}

/// Rolls the lead's releases back: the one before made live, from `current` and `previous` as
/// they were read (and checked: publish.sh's rollback_ok).
pub fn rollback(c: &Client, current: &str, previous: &str) -> Result<Asked> {
    asked(ROLLBACK, c.post_json(ROLLBACK, &serde_json::json!({ "current": current, "previous": previous }))?)
}

/// One check of this Mac's app against the lead's releases (jobrt::appupdate::check), the lead
/// reached as the NAS's contact says; with no lead there, nothing. The NAS's releases stand in while
/// the lead holds none (the rollout's).
pub fn check(home: &Path, root: &store::nasfs::NasRoot, running: &str) -> Result<Option<String>> {
    let Some(lead) = Client::from_nas(root, &store::sys::host_name())? else { return Ok(None) };
    jobrt::appupdate::check(home, &lead, Some(&jobrt::appupdate::NasReleases(root)), running)
}

/// Checks every five minutes on a thread of its own, from a program run from an installed
/// version (else nothing): the agent, which restarts into the new version by its update drain.
pub fn spawn(home: PathBuf, root: Option<store::nasfs::NasRoot>) {
    let Some(running) = jobrt::appupdate::running_version(&home) else { return };
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
