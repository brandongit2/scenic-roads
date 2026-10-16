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
    /// When the coordinator last answered (this Mac's clock, unix s; any answer but a refused
    /// token), and the pool's term and member it said it is (crate::coord::POOL_TERM, POOL_MEMBER).
    answered: Mutex<Option<Answered>>,
    /// When a request last reached none of the coordinator's addresses (this Mac's clock, unix s),
    /// and why (crate::pool::reachable: its cause read from it).
    failed: Mutex<Option<(u64, String)>>,
    /// Rounds (an agent's loop, `new_round`): None, not in rounds (every request tries); Some(None),
    /// in a round the coordinator hasn't failed to answer; Some(Some(why)), it didn't answer a
    /// request this round, so the round's later requests fail at once with why, rather than each
    /// waiting out its timeout. The next round tries again.
    round: Mutex<Option<Option<String>>>,
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

/// A coordinator's last answer to this client: when (this Mac's clock, unix s), and the pool's term
/// and member it said it is (None: it didn't say: not a pool's lead, or an older app).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answered {
    pub at: u64,
    pub term: Option<u64>,
    pub member: Option<String>,
}

/// Whether a request failed while connecting (resolving the name, or the connection timed out,
/// refused or unreachable), rather than once connected.
fn connect_phase(e: &ureq::Error) -> bool {
    use std::io::ErrorKind as K;
    match e {
        ureq::Error::Timeout(t) => matches!(t, ureq::Timeout::Resolve | ureq::Timeout::Connect),
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed => true,
        ureq::Error::Io(io) => matches!(io.kind(), K::ConnectionRefused | K::TimedOut | K::HostUnreachable | K::NetworkUnreachable | K::AddrNotAvailable),
        _ => false,
    }
}

/// Whether a coordinator (or a broker) answers at any of `urls`, trying each in turn with a short
/// timeout (crate::pool::reachable::PROBE_TIMEOUT_S): any HTTP answer counts, whatever its status
/// (it needs no token); else the last failure, its cause read from it.
pub fn probe(urls: &[String]) -> std::result::Result<(), (crate::pool::reachable::Cause, String)> {
    use crate::pool::reachable::{cause_of, Cause, PROBE_TIMEOUT_S};
    let t = Duration::from_secs(PROBE_TIMEOUT_S);
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_connect(Some(t)).timeout_global(Some(t)).http_status_as_error(false).build().into();
    let mut last = (Cause::Other, "no address".to_string());
    for url in urls {
        match agent.get(&format!("{url}/")).call() {
            Ok(_) => return Ok(()),
            Err(e) => {
                let why = format!("{url}: {e}");
                let cause = match &e {
                    ureq::Error::Timeout(_) => Cause::TimedOut,
                    ureq::Error::HostNotFound => Cause::NoName,
                    ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::ConnectionRefused => Cause::Refused,
                    ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut => Cause::TimedOut,
                    _ => cause_of(&why),
                };
                last = (cause, why);
            }
        }
    }
    Err(last)
}

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
        Ok(Some(Client { root: Some(root.to_path_buf()), contact: Mutex::new(c), worker: worker.to_string(), answered: Mutex::new(None), failed: Mutex::new(None), round: Mutex::new(None) }))
    }

    /// A coordinator at fixed addresses.
    pub fn at(urls: Vec<String>, token: String, worker: &str) -> Client {
        Client { root: None, contact: Mutex::new(Contact { urls, token, page: None }), worker: worker.to_string(), answered: Mutex::new(None), failed: Mutex::new(None), round: Mutex::new(None) }
    }

    /// Begins a round (an agent's loop): from now on, once a request gets no answer from the
    /// coordinator (none of its addresses answered), the round's later requests fail at once,
    /// without trying, until the next round begins.
    pub fn new_round(&self) {
        *self.round.lock().unwrap() = Some(None);
    }

    /// Whether the coordinator didn't answer a request this round (its later requests not tried).
    pub fn round_failed(&self) -> bool {
        matches!(&*self.round.lock().unwrap(), Some(Some(_)))
    }

    /// When the coordinator last answered a request, and as what (`Answered`); None: not yet.
    pub fn answered(&self) -> Option<Answered> {
        self.answered.lock().unwrap().clone()
    }

    /// When a request last failed to connect to any of its addresses, and why; None: none has.
    pub fn failed(&self) -> Option<(u64, String)> {
        self.failed.lock().unwrap().clone()
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
        if let Some(Some(why)) = &*self.round.lock().unwrap() {
            bail!("not tried: the coordinator didn't answer earlier in this round ({why})");
        }
        let mut unanswered = false;
        let r = self.request_now(method, path, body, &mut unanswered);
        // (No answer: every address failed while connecting or closed its connection unanswered,
        // or an answer broke off before it was whole. Not a connected lead's slow request run past
        // its time, nor any HTTP answer.)
        if let (Err(e), true) = (&r, unanswered) {
            let mut g = self.round.lock().unwrap();
            if let Some(down) = g.as_mut() {
                *down = Some(format!("{e:#}"));
            }
        }
        r
    }

    fn request_now(&self, method: &str, path: &str, body: Option<&[u8]>, unanswered: &mut bool) -> Result<(u16, Vec<u8>)> {
        let mut last = None;
        // (Whether every address failed while connecting: only that says it can't be reached, not
        // a long request that ran past its time.)
        let mut connecting = None;
        // (Whether no address answered at all: each failed while connecting, or closed or broke
        // its connection before an answer; not one that ran past its time once connected.)
        let mut silent = None;
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
                        let b = match resp.body_mut().with_config().limit(u64::MAX).read_to_vec() {
                            Ok(b) => b,
                            Err(e) => {
                                // (Broken off, not run past its time.)
                                *unanswered = !matches!(e, ureq::Error::Timeout(_));
                                return Err(anyhow::Error::from(e).context("the coordinator's answer"));
                            }
                        };
                        if code == 401 {
                            last = Some(anyhow::anyhow!("the coordinator refused the token"));
                            break;
                        }
                        let h = |n: &str| resp.headers().get(n).and_then(|v| v.to_str().ok()).map(str::to_string);
                        let term = h(super::POOL_TERM).and_then(|t| t.parse().ok());
                        *self.answered.lock().unwrap() = Some(Answered { at: crate::agent::jobs::now_s(), term, member: h(super::POOL_MEMBER) });
                        // The working address first from now on.
                        let mut cur = self.contact.lock().unwrap();
                        if let Some(i) = cur.urls.iter().position(|u| u == url) {
                            let u = cur.urls.remove(i);
                            cur.urls.insert(0, u);
                        }
                        return Ok((code, b));
                    }
                    Err(e) => {
                        connecting = Some(connecting.unwrap_or(true) && connect_phase(&e));
                        silent = Some(silent.unwrap_or(true) && (connect_phase(&e) || !matches!(e, ureq::Error::Timeout(_))));
                        last = Some(anyhow::Error::from(e).context(format!("reach the coordinator at {url}")));
                    }
                }
            }
        }
        let e = last.unwrap_or_else(|| anyhow::anyhow!("the coordinator has no address"));
        if silent == Some(true) {
            *unanswered = true;
        }
        if connecting == Some(true) {
            *self.failed.lock().unwrap() = Some((crate::agent::jobs::now_s(), format!("{e:#}")));
        }
        Err(e)
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
        let f = Fail { worker: self.worker.clone(), lease, error: error.chars().take(4000).collect(), oom_mb, interrupted: false, floors: floors.to_vec(), disks: Vec::new(), job_disk_mb: 0 };
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
        let f = Fail { worker: self.worker.clone(), lease, error: why.chars().take(4000).collect(), oom_mb: None, interrupted: true, floors: floors.to_vec(), disks: Vec::new(), job_disk_mb: 0 };
        self.post_json("/work/fail", &serde_json::to_value(&f)?)?;
        Ok(())
    }

    /// Ends lease `lease` unfinished: given back (`interrupted`) or failed, with what the job saw its
    /// targets take at least, in memory (`floors`) and on the disk (`disks`: cost key, MB).
    pub fn end_with(&self, lease: u64, why: &str, interrupted: bool, floors: &[(String, crate::agent::memguard::Floor)], disks: &[(String, u64)], job_disk_mb: u64) -> Result<()> {
        let f = Fail { worker: self.worker.clone(), lease, error: why.chars().take(4000).collect(), oom_mb: None, interrupted, floors: floors.to_vec(), disks: disks.to_vec(), job_disk_mb };
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A coordinator that never answers: each connection taken (counted, before it's closed) and
    /// closed without a word, so a request fails at once rather than waiting out a timeout.
    fn mute() -> (String, Arc<AtomicUsize>) {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let n = Arc::new(AtomicUsize::new(0));
        let seen = n.clone();
        std::thread::spawn(move || {
            for s in l.incoming() {
                seen.fetch_add(1, Ordering::SeqCst);
                drop(s);
            }
        });
        (url, n)
    }

    /// A coordinator answering every request with `status` and `{}`, counting them.
    pub(crate) fn answering(status: u16) -> (String, Arc<AtomicUsize>) {
        use std::io::{BufRead, BufReader, Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let n = Arc::new(AtomicUsize::new(0));
        let seen = n.clone();
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(mut s) = s else { continue };
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut len = 0usize;
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).ok();
                seen.fetch_add(1, Ordering::SeqCst);
                write!(s, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").ok();
            }
        });
        (url, n)
    }

    /// An address nothing listens at (a listener's, closed): a connection to it refused.
    pub(crate) fn dead() -> String {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    }

    #[test]
    fn a_refused_connection_fails_the_round() {
        let c = Client::at(vec![dead()], "t".into(), "m");
        c.new_round();
        assert!(c.beat(1, None).is_err());
        assert!(c.round_failed());
        assert!(c.failed().is_some());
    }

    #[test]
    fn any_http_answer_leaves_the_round_open() {
        for status in [500, 409, 401, 200] {
            let (url, tried) = answering(status);
            let c = Client::at(vec![url], "t".into(), "m");
            c.new_round();
            let _ = c.post_json("/work/x", &serde_json::json!({}));
            assert!(!c.round_failed(), "{status}");
            let _ = c.post_json("/work/x", &serde_json::json!({}));
            assert_eq!(tried.load(Ordering::SeqCst), 2, "{status}: both tried");
        }
    }

    #[test]
    fn one_address_answering_leaves_the_round_open() {
        let (url, tried) = answering(200);
        let c = Client::at(vec![dead(), url], "t".into(), "m");
        c.new_round();
        c.post_json("/work/x", &serde_json::json!({})).unwrap();
        assert!(!c.round_failed());
        c.post_json("/work/x", &serde_json::json!({})).unwrap();
        assert_eq!(tried.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_round_tries_an_unanswering_coordinator_once() {
        let (url, tried) = mute();
        let c = Client::at(vec![url], "t".into(), "m");
        c.new_round();
        assert!(c.beat(1, None).is_err());
        assert_eq!(tried.load(Ordering::SeqCst), 1);
        let e = c.post_json("/work/ask", &serde_json::json!({})).unwrap_err();
        assert!(format!("{e:#}").contains("not tried"), "{e:#}");
        assert!(c.done(&Done { lease: 1, ..Default::default() }).is_err());
        assert_eq!(tried.load(Ordering::SeqCst), 1, "the round's later requests not tried");
        // The next round tries again.
        c.new_round();
        assert!(c.beat(1, None).is_err());
        assert_eq!(tried.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn outside_rounds_every_request_tries() {
        let (url, tried) = mute();
        let c = Client::at(vec![url], "t".into(), "m");
        assert!(c.beat(1, None).is_err());
        assert!(c.beat(1, None).is_err());
        assert_eq!(tried.load(Ordering::SeqCst), 2);
    }
}
