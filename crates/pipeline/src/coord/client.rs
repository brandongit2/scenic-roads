//! A worker's side of the coordinator (a member's agent, a job offering tasks). It finds the
//! coordinator in `state/coordinator.json` on the NAS and reads that again whenever it can't reach
//! it, or the token is refused: the lead may have restarted, moved networks, or made a new
//! token.

use super::{Ask, Contact, Done, Fail, Grant};
use anyhow::{bail, Context, Result};
use std::sync::Mutex;
use std::time::Duration;

#[derive(Debug)]
pub struct Client {
    /// Where the contact is read from again (None: fixed addresses, in tests).
    root: Option<store::nasfs::NasRoot>,
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
    /// How a file's transfer is timed (`Files`).
    files: Files,
}

/// How a file's transfer (`/work/in/`, `/work/out/`) is timed: no limit on its whole time, so a big
/// file on a slow link goes on as long as it moves, but a connection that stalls fails. Connecting
/// has `connect`; after that, the request fails once `idle` passes with no byte received or sent
/// (counted from the connection made, each byte sent and each byte read: so it bounds the wait
/// for the answer's first byte too).
#[derive(Clone)]
pub struct Files {
    pub connect: Duration,
    pub idle: Duration,
    /// How often a blocked read wakes to check `idle` (a socket read timeout).
    pub tick: Duration,
    /// The clock `idle` is measured on: time since some fixed start (a test's own, to simulate time).
    pub clock: std::sync::Arc<dyn Fn() -> Duration + Send + Sync>,
}

impl Default for Files {
    fn default() -> Self {
        let t0 = std::time::Instant::now();
        Files {
            connect: Duration::from_secs(5),
            idle: Duration::from_secs(60),
            tick: Duration::from_secs(1),
            clock: std::sync::Arc::new(move || t0.elapsed()),
        }
    }
}

impl std::fmt::Debug for Files {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Files")
            .field("connect", &self.connect)
            .field("idle", &self.idle)
            .field("tick", &self.tick)
            .finish()
    }
}

/// A connector wrapping ureq's own, its transports failing a stalled transfer (`Files`).
#[derive(Debug)]
struct IdleConnector(Files);

impl ureq::unversioned::transport::Connector<Box<dyn ureq::unversioned::transport::Transport>> for IdleConnector {
    type Out = IdleTransport;

    fn connect(
        &self,
        _: &ureq::unversioned::transport::ConnectionDetails,
        chained: Option<Box<dyn ureq::unversioned::transport::Transport>>,
    ) -> std::result::Result<Option<IdleTransport>, ureq::Error> {
        Ok(chained.map(|inner| IdleTransport { last: (self.0.clock)(), inner, files: self.0.clone() }))
    }
}

/// A transport whose reads wake each `tick` and fail once nothing moved for `idle` (`Files`), and
/// whose writes each fail past `idle`.
#[derive(Debug)]
struct IdleTransport {
    inner: Box<dyn ureq::unversioned::transport::Transport>,
    files: Files,
    /// When a byte last moved (`Files::clock`).
    last: Duration,
}

impl IdleTransport {
    fn shorter(t: ureq::unversioned::transport::NextTimeout, d: Duration) -> ureq::unversioned::transport::NextTimeout {
        use ureq::unversioned::transport::{time, NextTimeout};
        if !t.after.is_not_happening() && *t.after <= d {
            t
        } else {
            NextTimeout { after: time::Duration::Exact(d), reason: t.reason }
        }
    }
}

impl ureq::unversioned::transport::Transport for IdleTransport {
    fn buffers(&mut self) -> &mut dyn ureq::unversioned::transport::Buffers {
        self.inner.buffers()
    }

    fn transmit_output(
        &mut self,
        amount: usize,
        timeout: ureq::unversioned::transport::NextTimeout,
    ) -> std::result::Result<(), ureq::Error> {
        // (A request starts with a write: a pooled connection's idle time before it isn't counted.
        // A write is at most ureq's output buffer, its progress counted when it's all written:
        // `idle` on it is a stall's limit.)
        self.last = (self.files.clock)();
        self.inner.transmit_output(amount, Self::shorter(timeout, self.files.idle))?;
        self.last = (self.files.clock)();
        Ok(())
    }

    fn await_input(
        &mut self,
        timeout: ureq::unversioned::transport::NextTimeout,
    ) -> std::result::Result<bool, ureq::Error> {
        loop {
            // (ureq's own limit, when it's sooner than a tick: as it says.)
            let own = !timeout.after.is_not_happening() && *timeout.after <= self.files.tick;
            match self.inner.await_input(Self::shorter(timeout, self.files.tick)) {
                Ok(moved) => {
                    self.last = (self.files.clock)();
                    return Ok(moved);
                }
                Err(ureq::Error::Timeout(why)) if !own => {
                    if (self.files.clock)().saturating_sub(self.last) >= self.files.idle {
                        // (Once more before failing: bytes that came while this thread waited to
                        // read the clock moved, so it's no stall.)
                        return match self.inner.await_input(Self::shorter(timeout, self.files.tick)) {
                            Ok(moved) => {
                                self.last = (self.files.clock)();
                                Ok(moved)
                            }
                            Err(ureq::Error::Timeout(_)) => Err(ureq::Error::Timeout(why)),
                            Err(e) => Err(e),
                        };
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn is_open(&mut self) -> bool {
        self.inner.is_open()
    }
}

/// What the coordinator answered: its status and its JSON (Null for none).
pub type Reply = (u16, serde_json::Value);

/// Work refused to this worker as it is (an agent on an older app than the lead's, or the build
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
        ureq::Error::Io(io) => matches!(
            io.kind(),
            K::ConnectionRefused | K::TimedOut | K::HostUnreachable | K::NetworkUnreachable | K::AddrNotAvailable
        ),
        _ => false,
    }
}

/// Whether a coordinator (or a broker) answers at any of `urls`, trying each in turn with a short
/// timeout (crate::pool::reachable::PROBE_TIMEOUT_S): any HTTP answer counts, whatever its status
/// (it needs no token); else the last failure, its cause read from it.
pub fn probe(urls: &[String]) -> std::result::Result<(), (crate::pool::reachable::Cause, String)> {
    use crate::pool::reachable::{cause_of, Cause, PROBE_TIMEOUT_S};
    let t = Duration::from_secs(PROBE_TIMEOUT_S);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(t))
        .timeout_global(Some(t))
        .http_status_as_error(false)
        .build()
        .into();
    let mut last = (Cause::Other, "no address".to_string());
    let _w = store::waits::net();
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
    pub fn from_nas(root: &store::nasfs::NasRoot, worker: &str) -> Result<Option<Client>> {
        let Some(c) = read_contact(root)? else { return Ok(None) };
        Ok(Some(Client {
            root: Some(root.clone()),
            contact: Mutex::new(c),
            worker: worker.to_string(),
            answered: Mutex::new(None),
            failed: Mutex::new(None),
            round: Mutex::new(None),
            files: Files::default(),
        }))
    }

    /// A coordinator at fixed addresses.
    pub fn at(urls: Vec<String>, token: String, worker: &str) -> Client {
        Client {
            root: None,
            contact: Mutex::new(Contact { urls, token, page: None }),
            worker: worker.to_string(),
            answered: Mutex::new(None),
            failed: Mutex::new(None),
            round: Mutex::new(None),
            files: Files::default(),
        }
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

    /// Times file transfers by `files` instead (tests: short limits, a simulated clock).
    pub fn with_files(mut self, files: Files) -> Client {
        self.files = files;
        self
    }

    fn is_file(path: &str) -> bool {
        path.starts_with("/work/in/") || path.starts_with("/work/out/")
    }

    /// The HTTP client for a request to `path`: a file (a task's input or output, `/work/in/`,
    /// `/work/out/`) has no limit on its whole time, only on a stall (`Files`); anything else 15 s
    /// in all, so a lead that doesn't answer (asleep, its agent stopped) holds no worker's loop up
    /// for long.
    /// (A fresh Agent per request: no connection is pooled between requests.)
    fn agent(&self, path: &str) -> ureq::Agent {
        if Self::is_file(path) {
            let f = &self.files;
            let config =
                ureq::Agent::config_builder().timeout_connect(Some(f.connect)).http_status_as_error(false).build();
            use ureq::unversioned::transport::Connector;
            return ureq::Agent::with_parts(
                config,
                ureq::unversioned::transport::DefaultConnector::new().chain(IdleConnector(f.clone())),
                ureq::unversioned::resolver::DefaultResolver::default(),
            );
        }
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(5)))
            .timeout_global(Some(Duration::from_secs(15)))
            .http_status_as_error(false)
            .build()
            .into()
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

    fn request_now(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
        unanswered: &mut bool,
    ) -> Result<(u16, Vec<u8>)> {
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
                    Ok(None) => bail!("the lead's coordinator is gone (no {})", super::contact_path(root).display()),
                    Err(e) => return Err(e),
                }
            }
            let c = self.contact.lock().unwrap().clone();
            let _w = store::waits::net();
            for url in &c.urls {
                let full = format!("{url}{path}");
                let auth = format!("Bearer {}", c.token);
                let r = match (method, body) {
                    ("GET", _) => self
                        .agent(path)
                        .get(&full)
                        .header("Authorization", &auth)
                        .header("X-Worker", &self.worker)
                        .call(),
                    ("PUT", Some(b)) => self
                        .agent(path)
                        .put(&full)
                        .header("Authorization", &auth)
                        .header("X-Worker", &self.worker)
                        .send(b),
                    (_, b) => self
                        .agent(path)
                        .post(&full)
                        .header("Authorization", &auth)
                        .header("X-Worker", &self.worker)
                        .header("Content-Type", "application/json")
                        .send(b.unwrap_or(b"{}")),
                };
                match r {
                    Ok(mut resp) => {
                        let code = resp.status().as_u16();
                        let b = match resp.body_mut().with_config().limit(u64::MAX).read_to_vec() {
                            Ok(b) => b,
                            Err(e) => {
                                // (Broken off, not run past its time.)
                                *unanswered = !matches!(e, ureq::Error::Timeout(_));
                                return Err(
                                    self.stalled(path, anyhow::Error::from(e).context("the coordinator's answer"))
                                );
                            }
                        };
                        if code == 401 {
                            last = Some(anyhow::anyhow!("the coordinator refused the token"));
                            break;
                        }
                        let h = |n: &str| resp.headers().get(n).and_then(|v| v.to_str().ok()).map(str::to_string);
                        let term = h(super::POOL_TERM).and_then(|t| t.parse().ok());
                        *self.answered.lock().unwrap() =
                            Some(Answered { at: crate::agent::jobs::now_s(), term, member: h(super::POOL_MEMBER) });
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
                        silent = Some(
                            silent.unwrap_or(true) && (connect_phase(&e) || !matches!(e, ureq::Error::Timeout(_))),
                        );
                        let stalled = !connect_phase(&e) && matches!(e, ureq::Error::Timeout(_));
                        let e = anyhow::Error::from(e).context(format!("reach the coordinator at {url}"));
                        last = Some(if stalled { self.stalled(path, e) } else { e });
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

    /// A file transfer's timeout said as what it is: nothing moved for `Files::idle`.
    fn stalled(&self, path: &str, e: anyhow::Error) -> anyhow::Error {
        if Self::is_file(path)
            && e.chain().any(|c| matches!(c.downcast_ref::<ureq::Error>(), Some(ureq::Error::Timeout(_))))
        {
            e.context(format!("{path}: no byte moved for {} s", self.files.idle.as_secs()))
        } else {
            e
        }
    }

    /// A JSON request: the status and the answer's JSON.
    pub fn post_json(&self, path: &str, body: &serde_json::Value) -> Result<Reply> {
        let (code, b) = self.request("POST", path, Some(&serde_json::to_vec(body)?))?;
        let v = if b.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&b).with_context(|| format!("the coordinator's answer to {path}"))?
        };
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
            (409, v) => Err(Refused {
                why: v["error"].as_str().unwrap_or("the lead gives this Mac no work as it is").to_string(),
                pause: serde_json::from_value(v["pause"].clone()).ok().flatten(),
            }
            .into()),
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
        self.beat_told(lease, progress, None, None).map(|(alive, pause, _)| (alive, pause))
    }

    /// `beat_paused`, with what its job is doing now (crate::agent::jobs::Live), and the owner's
    /// cancel of the lease's job (`scenic job cancel`), when there's one: the job stops and gives
    /// the lease back. `slot`: the slot of this Mac running its job (crate::coord::lease::Lease::slot).
    pub fn beat_told(
        &self,
        lease: u64,
        progress: Option<&str>,
        live: Option<crate::agent::jobs::Live>,
        slot: Option<u32>,
    ) -> Result<(bool, Option<crate::control::Pause>, Option<crate::control::Cancel>)> {
        let b = super::Beat {
            worker: self.worker.clone(),
            lease,
            progress: progress.map(str::to_string),
            frac: None,
            live,
            slot,
        };
        let v = self.post_json("/work/beat", &serde_json::to_value(&b)?)?.1;
        Ok((
            v["ok"].as_bool().unwrap_or(false),
            serde_json::from_value(v["pause"].clone()).ok().flatten(),
            serde_json::from_value(v["cancel"].clone()).ok().flatten(),
        ))
    }

    /// Passes on the owner's cancel (crate::control::Cancel) to the lead: the leases it told to stop,
    /// as (lease, worker, what).
    pub fn cancel(&self, c: &crate::control::Cancel) -> Result<Vec<(u64, String, String)>> {
        let v = self.post_json("/work/cancel", &serde_json::to_value(c)?)?.1;
        let s = |t: &serde_json::Value, k: &str| t[k].as_str().unwrap_or("").to_string();
        Ok(v["told"]
            .as_array()
            .map(|a| a.iter().map(|t| (t["lease"].as_u64().unwrap_or(0), s(t, "worker"), s(t, "what"))).collect())
            .unwrap_or_default())
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
        let why = || {
            serde_json::from_slice::<serde_json::Value>(&b)
                .ok()
                .and_then(|v| v["error"].as_str().map(str::to_string))
                .unwrap_or_default()
        };
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
    pub fn fail_with(
        &self,
        lease: u64,
        error: &str,
        oom_mb: Option<u64>,
        floors: &[(String, crate::agent::memguard::Floor)],
    ) -> Result<()> {
        let f = Fail {
            worker: self.worker.clone(),
            lease,
            error: error.chars().take(4000).collect(),
            oom_mb,
            interrupted: false,
            floors: floors.to_vec(),
            disks: Vec::new(),
            job_disk_mb: 0,
        };
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
    pub fn give_back_with(
        &self,
        lease: u64,
        why: &str,
        floors: &[(String, crate::agent::memguard::Floor)],
    ) -> Result<()> {
        let f = Fail {
            worker: self.worker.clone(),
            lease,
            error: why.chars().take(4000).collect(),
            oom_mb: None,
            interrupted: true,
            floors: floors.to_vec(),
            disks: Vec::new(),
            job_disk_mb: 0,
        };
        self.post_json("/work/fail", &serde_json::to_value(&f)?)?;
        Ok(())
    }

    /// Ends lease `lease` unfinished: given back (`interrupted`) or failed, with what the job saw its
    /// targets take at least, in memory (`floors`) and on the disk (`disks`: cost key, MB).
    pub fn end_with(
        &self,
        lease: u64,
        why: &str,
        interrupted: bool,
        floors: &[(String, crate::agent::memguard::Floor)],
        disks: &[(String, u64)],
        job_disk_mb: u64,
    ) -> Result<()> {
        let f = Fail {
            worker: self.worker.clone(),
            lease,
            error: why.chars().take(4000).collect(),
            oom_mb: None,
            interrupted,
            floors: floors.to_vec(),
            disks: disks.to_vec(),
            job_disk_mb,
        };
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
fn read_contact(root: &store::nasfs::NasRoot) -> Result<Option<Contact>> {
    match super::contact_path(root).read() {
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

    /// A test's listener (store::spawn's, not blocking) and its connections as they come, each
    /// blocking again, to `each` on its own thread's loop.
    fn serve(each: impl Fn(std::net::TcpStream) + Send + 'static) -> String {
        let l = store::spawn::listen("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        std::thread::spawn(move || loop {
            match store::spawn::accept(&l) {
                Ok((s, _)) => {
                    s.set_nonblocking(false).ok();
                    each(s);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(2))
                }
                Err(_) => {}
            }
        });
        url
    }

    /// A coordinator that never answers: each connection taken (counted, before it's closed) and
    /// closed without a word, so a request fails at once rather than waiting out a timeout.
    fn mute() -> (String, Arc<AtomicUsize>) {
        let n = Arc::new(AtomicUsize::new(0));
        let seen = n.clone();
        let url = serve(move |s| {
            seen.fetch_add(1, Ordering::SeqCst);
            drop(s);
        });
        (url, n)
    }

    /// A coordinator answering every request with `status` and `{}`, counting them.
    pub(crate) fn answering(status: u16) -> (String, Arc<AtomicUsize>) {
        use std::io::{BufRead, BufReader, Read, Write};
        let n = Arc::new(AtomicUsize::new(0));
        let seen = n.clone();
        let url = serve(move |mut s| {
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
        });
        (url, n)
    }

    /// An address nothing listens at (a listener's, closed): a connection to it refused.
    pub(crate) fn dead() -> String {
        let l = store::spawn::listen("127.0.0.1:0").unwrap();
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

    /// Reads a request's head (and drops its body), as `answering` does.
    fn read_head(s: &std::net::TcpStream) {
        use std::io::{BufRead, BufReader, Read};
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
    }

    /// File limits on a simulated clock (ms, as `clock` holds it): the real ones (5 s, 60 s), a
    /// blocked read waking every 5 ms; `auto`: each look at the clock moves it on 1 s.
    fn sim(clock: Arc<std::sync::atomic::AtomicU64>, auto: bool) -> Files {
        let clock = move || {
            Duration::from_millis(if auto {
                clock.fetch_add(1000, Ordering::SeqCst)
            } else {
                clock.load(Ordering::SeqCst)
            })
        };
        Files { tick: Duration::from_millis(5), clock: Arc::new(clock), ..Files::default() }
    }

    #[test]
    fn a_slow_but_steady_file_arrives_however_long_it_takes() {
        use std::io::Write;
        let clock = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let c2 = clock.clone();
        // 30 pieces, 10 simulated seconds apart: 5 minutes in all, no gap near the idle limit.
        let url = serve(move |mut s| {
            read_head(&s);
            write!(s, "HTTP/1.1 200 OK\r\nContent-Length: 30000\r\nConnection: close\r\n\r\n").unwrap();
            for i in 0..30u8 {
                s.write_all(&[i; 1000]).unwrap();
                s.flush().unwrap();
                c2.fetch_add(10_000, Ordering::SeqCst);
            }
        });
        let c = Client::at(vec![url], "t".into(), "m").with_files(sim(clock.clone(), false));
        let b = c.get_bytes("/work/in/1/u/big.bin").unwrap();
        assert_eq!(b.len(), 30000);
        assert!(b.chunks(1000).enumerate().all(|(i, p)| p.iter().all(|&x| x == i as u8)));
        assert!(clock.load(Ordering::SeqCst) >= 300_000, "past the old 2 minutes");
    }

    #[test]
    fn a_stalled_file_fails_at_the_idle_limit() {
        use std::io::Write;
        // Stalled mid-body, and before any answer.
        for head in [true, false] {
            let (hold, held) = std::sync::mpsc::channel::<()>();
            let held = Mutex::new(held);
            let url = serve(move |mut s| {
                read_head(&s);
                if head {
                    write!(s, "HTTP/1.1 200 OK\r\nContent-Length: 2000\r\nConnection: close\r\n\r\n").unwrap();
                    s.write_all(&[7; 1000]).unwrap();
                    s.flush().unwrap();
                }
                // (Held open, silent, until the test ends.)
                let _ = held.lock().unwrap().recv();
            });
            let clock = Arc::new(std::sync::atomic::AtomicU64::new(0));
            let c = Client::at(vec![url], "t".into(), "m").with_files(sim(clock.clone(), true));
            c.new_round();
            let e = c.get_bytes("/work/in/1/u/big.bin").unwrap_err();
            assert!(format!("{e:#}").contains("no byte moved for 60 s"), "{head}: {e:#}");
            let t = clock.load(Ordering::SeqCst);
            assert!((60_000..70_000).contains(&t), "{head}: failed at {t} ms simulated");
            assert!(!c.round_failed(), "{head}: a connected coordinator that stalled isn't one that didn't answer");
            drop(hold);
        }
    }

    #[test]
    fn requests_other_than_files_keep_their_15_s() {
        // (Not a file: the idle transport isn't used, whatever its clock says.)
        let (url, _) = answering(200);
        let clock = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let c = Client::at(vec![url], "t".into(), "m").with_files(sim(clock.clone(), true));
        c.post_json("/work/x", &serde_json::json!({})).unwrap();
        assert_eq!(clock.load(Ordering::SeqCst), 0);
    }
}
