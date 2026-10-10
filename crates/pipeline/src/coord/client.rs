//! A worker's side of the coordinator (the M1's agent, a job offering tasks). It finds the
//! coordinator in `state/coordinator.json` on the NAS and reads that again whenever it can't reach
//! it, or the token is refused: the build Mac may have restarted, moved networks, or made a new
//! token.

use super::{Ask, Contact, Done, Fail, Grant};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

#[derive(Debug)]
pub struct Client {
    /// Where the contact is read from again (None: fixed addresses, in tests).
    root: Option<PathBuf>,
    contact: Mutex<Contact>,
    /// This worker's name.
    pub worker: String,
}

/// What the coordinator answered: its status and its JSON (Null for none).
pub type Reply = (u16, serde_json::Value);

/// Work refused to this worker as it is (an agent on an older app than the build Mac's, or the build
/// paused): why, in words for its status, and the build's pause, when that's why.
#[derive(Debug)]
pub struct Refused {
    pub why: String,
    pub pause: Option<crate::control::Pause>,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.why)
    }
}

impl std::error::Error for Refused {}

/// What became of work handed back (`Client::done`).
#[derive(Debug, PartialEq, Eq)]
pub enum Handed {
    Taken,
    Gone,
    Refused(String),
}

impl Client {
    /// The coordinator as the NAS says to reach it; None when there's none (no file), an error when
    /// the file can't be read now (an SMB hiccup: not the same as none).
    pub fn from_nas(root: &Path, worker: &str) -> Result<Option<Client>> {
        let Some(c) = read_contact(root)? else { return Ok(None) };
        Ok(Some(Client { root: Some(root.to_path_buf()), contact: Mutex::new(c), worker: worker.to_string() }))
    }

    /// A coordinator at fixed addresses.
    pub fn at(urls: Vec<String>, token: String, worker: &str) -> Client {
        Client { root: None, contact: Mutex::new(Contact { urls, token, page: None }), worker: worker.to_string() }
    }

    pub fn urls(&self) -> Vec<String> {
        self.contact.lock().unwrap().urls.clone()
    }

    pub fn token(&self) -> String {
        self.contact.lock().unwrap().token.clone()
    }

    /// The HTTP client for a request to `path`: a file (a task's input or output, `/work/in/`,
    /// `/work/out/`) has two minutes; anything else 15 s, so a build Mac that doesn't answer (asleep,
    /// its agent stopped) holds no worker's loop up for long.
    fn agent(path: &str) -> ureq::Agent {
        let files = path.starts_with("/work/in/") || path.starts_with("/work/out/");
        ureq::Agent::config_builder().timeout_connect(Some(Duration::from_secs(5))).timeout_global(Some(Duration::from_secs(if files { 120 } else { 15 }))).http_status_as_error(false).build().into()
    }

    /// One request, at each address in turn until one answers; once more after reading the contact
    /// again when none did or the token was refused.
    fn request(&self, method: &str, path: &str, body: Option<&[u8]>) -> Result<(u16, Vec<u8>)> {
        let mut last = None;
        for fresh in [false, true] {
            if fresh {
                let Some(root) = &self.root else { break };
                match read_contact(root) {
                    Ok(Some(c)) => *self.contact.lock().unwrap() = c,
                    Ok(None) => bail!("the build Mac's coordinator is gone (no {})", super::contact_path(root).display()),
                    Err(e) => return Err(e),
                }
            }
            let c = self.contact.lock().unwrap().clone();
            for url in &c.urls {
                let full = format!("{url}{path}");
                let auth = format!("Bearer {}", c.token);
                let r = match (method, body) {
                    ("GET", _) => Self::agent(path).get(&full).header("Authorization", &auth).header("X-Worker", &self.worker).call(),
                    ("PUT", Some(b)) => Self::agent(path).put(&full).header("Authorization", &auth).header("X-Worker", &self.worker).send(b),
                    (_, b) => Self::agent(path).post(&full).header("Authorization", &auth).header("X-Worker", &self.worker).header("Content-Type", "application/json").send(b.unwrap_or(b"{}")),
                };
                match r {
                    Ok(mut resp) => {
                        let code = resp.status().as_u16();
                        let b = resp.body_mut().with_config().limit(u64::MAX).read_to_vec().context("the coordinator's answer")?;
                        if code == 401 {
                            last = Some(anyhow::anyhow!("the coordinator refused the token"));
                            break;
                        }
                        // The working address first from now on.
                        let mut cur = self.contact.lock().unwrap();
                        if let Some(i) = cur.urls.iter().position(|u| u == url) {
                            let u = cur.urls.remove(i);
                            cur.urls.insert(0, u);
                        }
                        return Ok((code, b));
                    }
                    Err(e) => last = Some(anyhow::Error::from(e).context(format!("reach the coordinator at {url}"))),
                }
            }
        }
        Err(last.unwrap_or_else(|| anyhow::anyhow!("the coordinator has no address")))
    }

    /// A JSON request: the status and the answer's JSON.
    pub fn post_json(&self, path: &str, body: &serde_json::Value) -> Result<Reply> {
        let (code, b) = self.request("POST", path, Some(&serde_json::to_vec(body)?))?;
        let v = if b.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&b).with_context(|| format!("the coordinator's answer to {path}"))? };
        // (404, 409 and 410 say something to the caller: no such thing; refused as it is; gone.)
        if code >= 400 && ![404, 409, 410].contains(&code) {
            bail!("the coordinator answered {code} to {path}: {}", v["error"].as_str().unwrap_or(""));
        }
        Ok((code, v))
    }

    /// Work that fits this worker; None when there's none now; an error, `Refused`, when none goes to
    /// it as it is.
    pub fn ask(&self, a: &Ask) -> Result<Option<Grant>> {
        let a = Ask { worker: self.worker.clone(), ..a.clone() };
        match self.post_json("/work/ask", &serde_json::to_value(&a)?)? {
            (200, v) => Ok(Some(serde_json::from_value(v)?)),
            (409, v) => Err(Refused { why: v["error"].as_str().unwrap_or("the build Mac gives this Mac no work as it is").to_string(), pause: serde_json::from_value(v["pause"].clone()).ok().flatten() }.into()),
            _ => Ok(None),
        }
    }

    /// Keeps lease `lease` alive; false when the coordinator no longer holds it for this worker (the
    /// work should stop: it was offered again).
    pub fn beat(&self, lease: u64, progress: Option<&str>) -> Result<bool> {
        Ok(self.beat_paused(lease, progress)?.0)
    }

    /// `beat`, and the build's pause, while it's paused (the job pauses with it).
    pub fn beat_paused(&self, lease: u64, progress: Option<&str>) -> Result<(bool, Option<crate::control::Pause>)> {
        let b = super::Beat { worker: self.worker.clone(), lease, progress: progress.map(str::to_string), frac: None };
        let v = self.post_json("/work/beat", &serde_json::to_value(&b)?)?.1;
        Ok((v["ok"].as_bool().unwrap_or(false), serde_json::from_value(v["pause"].clone()).ok().flatten()))
    }

    /// Passes on this Mac's ask (crate::control::Request): the build paused, or going on, as asked
    /// at `at` (unix seconds: an ask older than the build's last change is passed over).
    pub fn set_pause(&self, pause: Option<&crate::control::Pause>, at: u64) -> Result<()> {
        self.post_json("/work/pause", &serde_json::json!({ "pause": pause, "at": at }))?;
        Ok(())
    }

    /// Hands work back: taken; gone (its lease ended: drop the work, it was offered again, and a
    /// late hand-off could undo a newer build); or refused (not what the lease asked for: give the
    /// lease back as failed, and drop the work).
    pub fn done(&self, d: &Done) -> Result<Handed> {
        let mut v = serde_json::to_value(d)?;
        v["worker"] = self.worker.clone().into();
        let (code, b) = self.request("POST", "/work/done", Some(&serde_json::to_vec(&v)?))?;
        let why = || serde_json::from_slice::<serde_json::Value>(&b).ok().and_then(|v| v["error"].as_str().map(str::to_string)).unwrap_or_default();
        match code {
            200 => Ok(Handed::Taken),
            410 => Ok(Handed::Gone),
            422 => Ok(Handed::Refused(why())),
            c => bail!("the coordinator answered {c} to /work/done: {}", why()),
        }
    }

    /// Gives lease `lease` back, failed (`oom_mb`: a task out of memory at that peak).
    pub fn fail(&self, lease: u64, error: &str, oom_mb: Option<u64>) -> Result<()> {
        self.fail_with(lease, error, oom_mb, &[])
    }

    /// `fail`, with what the job saw its targets hold at least (crate::agent::memguard: cost key,
    /// MB), for the lease's floors.
    pub fn fail_with(&self, lease: u64, error: &str, oom_mb: Option<u64>, floors: &[(String, crate::agent::memguard::Floor)]) -> Result<()> {
        let f = Fail { worker: self.worker.clone(), lease, error: error.chars().take(4000).collect(), oom_mb, interrupted: false, floors: floors.to_vec() };
        self.post_json("/work/fail", &serde_json::to_value(&f)?)?;
        Ok(())
    }

    /// Gives lease `lease` back unfinished, not failed (`why`: the build paused, the Mac slept, its
    /// agent restarted, it couldn't start here): its targets aren't kept from this worker.
    pub fn give_back(&self, lease: u64, why: &str) -> Result<()> {
        self.give_back_with(lease, why, &[])
    }

    /// `give_back`, with what the job saw its targets hold at least (crate::agent::memguard: cost
    /// key, MB), for the lease's floors.
    pub fn give_back_with(&self, lease: u64, why: &str, floors: &[(String, crate::agent::memguard::Floor)]) -> Result<()> {
        let f = Fail { worker: self.worker.clone(), lease, error: why.chars().take(4000).collect(), oom_mb: None, interrupted: true, floors: floors.to_vec() };
        self.post_json("/work/fail", &serde_json::to_value(&f)?)?;
        Ok(())
    }

    /// A file (a task's input).
    pub fn get_bytes(&self, path: &str) -> Result<Vec<u8>> {
        match self.request("GET", path, None)? {
            (200, b) => Ok(b),
            (c, b) => bail!("{path}: {c} {}", String::from_utf8_lossy(&b)),
        }
    }

    /// Sends a file (a task's output).
    pub fn put_bytes(&self, path: &str, data: &[u8]) -> Result<()> {
        match self.request("PUT", path, Some(data))? {
            (200, _) => Ok(()),
            (c, b) => bail!("{path}: {c} {}", String::from_utf8_lossy(&b)),
        }
    }
}

/// The contact on the NAS: None when there's no file, an error when it can't be read now.
fn read_contact(root: &Path) -> Result<Option<Contact>> {
    match std::fs::read(super::contact_path(root)) {
        Ok(b) => Ok(Some(serde_json::from_slice(&b).context("the coordinator's contact")?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).context("read the coordinator's contact"),
    }
}
