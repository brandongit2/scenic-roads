//! A worker's side of the coordinator (a member's agent, a job offering tasks). It finds the
//! coordinator in `state/coordinator.json` on the NAS and reads that again whenever it can't reach
//! it, or the token is refused: the lead may have restarted, moved networks, or made a new
//! token.
//!
//! Its connections are kept alive (`Agents`): one HTTP client for the whole process, pooling its
//! connections to each address, so a member beating every second uses one connection, not one a
//! request (each one closed held its port for 30 s in TIME_WAIT: a test program's many requests ran
//! out of ports, task #958). A kept connection gone stale is let go: one the coordinator closed is
//! tried again at once on a new connection; one whose Mac went silent (asleep, off the network) is
//! found out by connecting to it anew once the answer is `SILENT` that long, failing as a new
//! request's connection would (`Conn`).
//!
//! The requests here are the transport's (`post_json`, `request`, the files'); the lease calls
//! (ask, beat, done, fail, give back, cancel, pause, the pool's beat) carry the coordinator's own
//! types and are pipeline's (coord::client::LeaseCalls, on this `Client`).

use super::Contact;
use anyhow::{bail, Context, Result};
use std::cell::Cell;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct Client {
    /// Where the contact is read from again (None: fixed addresses, in tests).
    root: Option<store::nasfs::NasRoot>,
    contact: Mutex<Contact>,
    /// This worker's name.
    pub worker: String,
    /// When the coordinator last answered (this Mac's clock, unix s; any answer but a refused
    /// token), and the pool's term and member it said it is (crate::POOL_TERM, POOL_MEMBER).
    answered: Mutex<Option<Answered>>,
    /// When a request last reached none of the coordinator's addresses (this Mac's clock, unix s),
    /// and why (pipeline's pool::reachable: its cause read from it).
    failed: Mutex<Option<(u64, String)>>,
    /// Rounds (an agent's loop, `new_round`): None, not in rounds (every request tries); Some(None),
    /// in a round the coordinator hasn't failed to answer; Some(Some(why)), it didn't answer a
    /// request this round, so the round's later requests fail at once with why, rather than each
    /// waiting out its timeout. The next round tries again.
    round: Mutex<Option<Option<String>>>,
    /// Its HTTP clients: the process's (`Agents::shared`), or a test's own (`with_files`).
    agents: Arc<Agents>,
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

/// How long a kept-alive connection may sit idle before it's let go (ureq's pool), and how many are
/// kept: under the coordinator's own idle limit (`http::HEADERS_MAX`, 20 s), so the client lets one
/// go before the coordinator closes it.
const IDLE: Duration = Duration::from_secs(10);
const IDLE_PER_ADDRESS: usize = 4;
const IDLE_ALL: usize = 64;
/// A request on a kept-alive connection answered nothing for this long: its address is connected to
/// anew (`reach`), telling a lead that's slow from one that's gone. It only decides when to look,
/// never fails a request (a slow lead's costs one connection more): any time well over a tailnet
/// round trip (a relayed one, away, under half a second) and well under the request's 15 s does.
const SILENT: Duration = Duration::from_secs(2);
/// Connecting: a request's own connection, and `reach`'s.
const CONNECT: Duration = Duration::from_secs(5);

/// A `Client`'s HTTP clients: one for requests other than files, one for files (`Files`).
#[derive(Debug)]
struct Agents {
    plain: ureq::Agent,
    files: ureq::Agent,
    limits: Files,
}

impl Agents {
    fn new(files: Files) -> Agents {
        use ureq::unversioned::transport::Connector;
        let agent = |b: ureq::config::ConfigBuilder<ureq::typestate::AgentScope>, files: Option<Files>, connect| {
            let config = b
                .timeout_connect(Some(connect))
                .http_status_as_error(false)
                .max_idle_age(IDLE)
                .max_idle_connections_per_host(IDLE_PER_ADDRESS)
                .max_idle_connections(IDLE_ALL)
                .build();
            ureq::Agent::with_parts(
                config,
                ureq::unversioned::transport::DefaultConnector::new().chain(Kept { files, connect }),
                ureq::unversioned::resolver::DefaultResolver::default(),
            )
        };
        Agents {
            // (15 s in all, so a lead that doesn't answer (asleep, its agent stopped) holds no
            // worker's loop up for long. A file: no limit on its whole time, only on a stall.)
            plain: agent(ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(15))), None, CONNECT),
            files: agent(ureq::Agent::config_builder(), Some(files.clone()), files.connect),
            limits: files,
        }
    }

    /// The process's, which every `Client` shares, and their connections with them.
    fn shared() -> Arc<Agents> {
        static SHARED: OnceLock<Arc<Agents>> = OnceLock::new();
        SHARED.get_or_init(|| Arc::new(Agents::new(Files::default()))).clone()
    }
}

/// How this thread's last request on a connection (`Conn`) went, read when it fails: whether its
/// connection was a kept-alive one; whether the coordinator may have it (a byte of its answer came,
/// or its address took a new connection while it waited: `reach`); and whether its address took
/// none (`reach`: gone).
#[derive(Clone, Copy, Debug, Default)]
struct Use {
    reused: bool,
    heard: bool,
    gone: bool,
}

impl Use {
    /// Whether a request that failed `e` goes again on a new connection: its kept-alive one closed
    /// before any answer, so the coordinator never had it.
    fn again(&self, e: &ureq::Error) -> bool {
        self.reused && !self.heard && !self.gone && !matches!(e, ureq::Error::Timeout(_))
    }
}

thread_local! {
    // (ureq runs a request on its caller's thread, its connection's reads and writes too.)
    static USE: Cell<Use> = Cell::new(Use::default());
}

/// A connector wrapping ureq's own: each of its connections (`Conn`) says how its requests went
/// (`USE`) and finds a silent address gone (`SILENT`); a file's also fails a stalled transfer
/// (`Files`).
#[derive(Debug)]
struct Kept {
    files: Option<Files>,
    connect: Duration,
}

impl ureq::unversioned::transport::Connector<Box<dyn ureq::unversioned::transport::Transport>> for Kept {
    type Out = Conn;

    fn connect(
        &self,
        details: &ureq::unversioned::transport::ConnectionDetails,
        chained: Option<Box<dyn ureq::unversioned::transport::Transport>>,
    ) -> std::result::Result<Option<Conn>, ureq::Error> {
        Ok(chained.map(|inner| Conn {
            last: self.files.as_ref().map(|f| (f.clock)()).unwrap_or_default(),
            inner,
            files: self.files.clone(),
            addrs: details.addrs.to_vec(),
            connect: self.connect,
            requests: 0,
            reading: false,
        }))
    }
}

/// A connection (`Kept`).
#[derive(Debug)]
struct Conn {
    inner: Box<dyn ureq::unversioned::transport::Transport>,
    /// A file's limits: reads wake each `tick` and fail once nothing moved for `idle`; writes each
    /// fail past `idle`.
    files: Option<Files>,
    /// When a byte last moved (`Files::clock`).
    last: Duration,
    /// Its address, for `reach`.
    addrs: Vec<std::net::SocketAddr>,
    connect: Duration,
    /// The requests begun on it, and whether the last one is reading its answer (a write after that
    /// begins the next).
    requests: u32,
    reading: bool,
}

fn shorter(t: ureq::unversioned::transport::NextTimeout, d: Duration) -> ureq::unversioned::transport::NextTimeout {
    use ureq::unversioned::transport::{time, NextTimeout};
    if within(&t, d) {
        t
    } else {
        NextTimeout { after: time::Duration::Exact(d), reason: t.reason }
    }
}

/// Whether `t` (ureq's own limit) ends within `d`.
fn within(t: &ureq::unversioned::transport::NextTimeout, d: Duration) -> bool {
    !t.after.is_not_happening() && *t.after <= d
}

/// Whether any of `addrs` takes a new connection within `connect`; else the last failure.
fn reach(addrs: &[std::net::SocketAddr], connect: Duration) -> std::io::Result<()> {
    let mut last = std::io::Error::new(std::io::ErrorKind::AddrNotAvailable, "no address");
    for a in addrs {
        match std::net::TcpStream::connect_timeout(a, connect) {
            Ok(_) => return Ok(()),
            Err(e) => last = e,
        }
    }
    Err(last)
}

impl Conn {
    fn moved(&mut self) {
        if let Some(f) = &self.files {
            self.last = (f.clock)();
        }
    }

    /// Waits for input as a file's limits have it (`files`).
    fn await_file(
        &mut self,
        f: &Files,
        timeout: ureq::unversioned::transport::NextTimeout,
    ) -> std::result::Result<bool, ureq::Error> {
        // (ureq's own limit, when it's sooner than a tick: as it says.)
        let own = within(&timeout, f.tick);
        loop {
            match self.inner.await_input(shorter(timeout, f.tick)) {
                Err(ureq::Error::Timeout(why)) if !own => {
                    if (f.clock)().saturating_sub(self.last) >= f.idle {
                        // (Once more before failing: bytes that came while this thread waited to
                        // read the clock moved, so it's no stall.)
                        return match self.inner.await_input(shorter(timeout, f.tick)) {
                            Err(ureq::Error::Timeout(_)) => Err(ureq::Error::Timeout(why)),
                            r => r,
                        };
                    }
                }
                r => return r,
            }
        }
    }
}

impl ureq::unversioned::transport::Transport for Conn {
    fn buffers(&mut self) -> &mut dyn ureq::unversioned::transport::Buffers {
        self.inner.buffers()
    }

    fn transmit_output(
        &mut self,
        amount: usize,
        timeout: ureq::unversioned::transport::NextTimeout,
    ) -> std::result::Result<(), ureq::Error> {
        if self.requests == 0 || self.reading {
            USE.set(Use { reused: self.requests > 0, ..Use::default() });
            self.requests += 1;
            self.reading = false;
        }
        // (A request starts with a write: a kept connection's idle time before it isn't counted.
        // A write is at most ureq's output buffer, its progress counted when it's all written: a
        // file's `idle` on it is a stall's limit.)
        self.moved();
        let timeout = match &self.files {
            Some(f) => shorter(timeout, f.idle),
            None => timeout,
        };
        self.inner.transmit_output(amount, timeout)?;
        self.moved();
        Ok(())
    }

    fn await_input(
        &mut self,
        mut timeout: ureq::unversioned::transport::NextTimeout,
    ) -> std::result::Result<bool, ureq::Error> {
        self.reading = true;
        let u = USE.get();
        // A kept connection's request with no answer yet: once silent for `SILENT`, its address is
        // connected to anew; taking none, the request fails as a new connection to it would.
        if u.reused && !u.heard && !within(&timeout, SILENT) {
            let t0 = Instant::now();
            match self.inner.await_input(shorter(timeout, SILENT)) {
                Err(ureq::Error::Timeout(_)) => match reach(&self.addrs, self.connect) {
                    Err(e) => {
                        USE.set(Use { gone: true, ..u });
                        return Err(ureq::Error::Io(e));
                    }
                    Ok(()) => {
                        // (There, and slow: the rest of the request's time, as without the look.)
                        USE.set(Use { heard: true, ..u });
                        if !timeout.after.is_not_happening() {
                            timeout.after = ureq::unversioned::transport::time::Duration::Exact(
                                timeout.after.saturating_sub(t0.elapsed()),
                            );
                        }
                    }
                },
                r => return self.input(r),
            }
        }
        let r = match self.files.clone() {
            Some(f) => self.await_file(&f, timeout),
            None => self.inner.await_input(timeout),
        };
        self.input(r)
    }

    fn is_open(&mut self) -> bool {
        self.inner.is_open()
    }
}

impl Conn {
    /// A wait's outcome `r`, a byte that came counted.
    fn input(&mut self, r: std::result::Result<bool, ureq::Error>) -> std::result::Result<bool, ureq::Error> {
        if let Ok(true) = r {
            USE.set(Use { heard: true, ..USE.get() });
            self.moved();
        }
        r
    }
}

/// What the coordinator answered: its status and its JSON (Null for none).
pub type Reply = (u16, serde_json::Value);
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
            agents: Agents::shared(),
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
            agents: Agents::shared(),
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

    /// Its build page over HTTPS, when its contact says one (`Contact::page`).
    pub fn page(&self) -> Option<String> {
        self.contact.lock().unwrap().page.clone()
    }

    /// Times file transfers by `files` instead (tests: short limits, a simulated clock), on HTTP
    /// clients of its own.
    pub fn with_files(mut self, files: Files) -> Client {
        self.agents = Arc::new(Agents::new(files));
        self
    }

    fn is_file(path: &str) -> bool {
        path.starts_with("/work/in/") || path.starts_with("/work/out/") || path == "/map/snapshot"
    }

    /// The HTTP client for a request to `path`: a file's (a task's input or output, `/work/in/`,
    /// `/work/out/`, the map whole: `/map/snapshot`), or the other requests' (`Agents`).
    fn agent(&self, path: &str) -> &ureq::Agent {
        if Self::is_file(path) {
            &self.agents.files
        } else {
            &self.agents.plain
        }
    }

    /// One try of a request at `url`, on a kept connection or (`anew`) a new one.
    fn send(
        &self,
        method: &str,
        url: &str,
        path: &str,
        token: &str,
        body: Option<&[u8]>,
        anew: bool,
    ) -> std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error> {
        let full = format!("{url}{path}");
        let auth = format!("Bearer {token}");
        let agent = self.agent(path);
        // (No kept connection is old enough to be taken: a new one.)
        let idle = if anew { Duration::ZERO } else { IDLE };
        USE.set(Use::default());
        match (method, body) {
            ("GET", _) => agent
                .get(&full)
                .config()
                .max_idle_age(idle)
                .build()
                .header("Authorization", &auth)
                .header("X-Worker", &self.worker)
                .call(),
            ("PUT", Some(b)) => agent
                .put(&full)
                .config()
                .max_idle_age(idle)
                .build()
                .header("Authorization", &auth)
                .header("X-Worker", &self.worker)
                .send(b),
            (_, b) => agent
                .post(&full)
                .config()
                .max_idle_age(idle)
                .build()
                .header("Authorization", &auth)
                .header("X-Worker", &self.worker)
                .header("Content-Type", "application/json")
                .send(b.unwrap_or(b"{}")),
        }
    }

    /// One request, at each address in turn until one answers; once more after reading the contact
    /// again when none did or the token was refused. Its status and body.
    pub fn request(&self, method: &str, path: &str, body: Option<&[u8]>) -> Result<(u16, Vec<u8>)> {
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
                let mut r = self.send(method, url, path, &c.token, body, false);
                // (A kept connection the coordinator closed before it read the request: again, on a
                // new one.)
                if r.as_ref().err().is_some_and(|e| USE.get().again(e)) {
                    r = self.send(method, url, path, &c.token, body, true);
                }
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
                            Some(Answered { at: jobrt::jobs::now_s(), term, member: h(super::POOL_MEMBER) });
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
            *self.failed.lock().unwrap() = Some((jobrt::jobs::now_s(), format!("{e:#}")));
        }
        Err(e)
    }

    /// A file transfer's timeout said as what it is: nothing moved for `Files::idle`.
    fn stalled(&self, path: &str, e: anyhow::Error) -> anyhow::Error {
        if Self::is_file(path)
            && e.chain().any(|c| matches!(c.downcast_ref::<ureq::Error>(), Some(ureq::Error::Timeout(_))))
        {
            e.context(format!("{path}: no byte moved for {} s", self.agents.limits.idle.as_secs()))
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

    /// A file (a task's input).
    pub fn get_bytes(&self, path: &str) -> Result<Vec<u8>> {
        match self.request("GET", path, None)? {
            (200, b) => Ok(b),
            (c, b) => bail!("{path}: {c} {}", String::from_utf8_lossy(&b)),
        }
    }

    /// `get_bytes`, None when the coordinator answers that it can't now (503: the map's log not
    /// open yet, docs/maplog.md §5.1).
    pub fn get_ready(&self, path: &str) -> Result<Option<Vec<u8>>> {
        match self.request("GET", path, None)? {
            (200, b) => Ok(Some(b)),
            (503, _) => Ok(None),
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
mod tests {
    use super::*;
    use crate::testkit::{answering, dead, serve};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

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

    /// A coordinator keeping its connections alive: each connection's first request answered,
    /// `then` the next on it (`true`: closed unanswered, as a coordinator closing it idle just as
    /// it came; `false`: held silent, as a Mac gone asleep), its listener closed after the first
    /// connection (`close`: none taken after, so a new connection is refused). The connections
    /// taken, counted.
    fn keeping(then: bool, close: bool) -> (String, Arc<AtomicUsize>, std::sync::mpsc::Sender<()>) {
        use std::io::Write;
        let l = store::spawn::listen("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let seen = Arc::new(AtomicUsize::new(0));
        let taken = seen.clone();
        let (hold, held) = std::sync::mpsc::channel::<()>();
        let held = Arc::new(Mutex::new(held));
        std::thread::spawn(move || {
            let mut l = Some(l);
            while let Some(listener) = &l {
                let s = match store::spawn::accept(listener) {
                    Ok((s, _)) => s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(_) => continue,
                };
                taken.fetch_add(1, Ordering::SeqCst);
                if close {
                    store::spawn::sockets(|| l = None);
                }
                let held = held.clone();
                std::thread::spawn(move || {
                    let mut s = s;
                    s.set_nonblocking(false).ok();
                    read_head(&s);
                    write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}")
                        .unwrap();
                    read_head(&s);
                    if !then {
                        let _ = held.lock().unwrap().recv();
                    }
                });
            }
        });
        (url, seen, hold)
    }

    #[test]
    fn a_kept_connection_closed_unread_goes_again_on_a_new_one() {
        let (url, taken, _hold) = keeping(true, false);
        let c = Client::at(vec![url], "t".into(), "m");
        c.new_round();
        c.post_json("/work/x", &serde_json::json!({})).unwrap();
        c.post_json("/work/x", &serde_json::json!({})).unwrap();
        assert!(!c.round_failed());
        assert_eq!(taken.load(Ordering::SeqCst), 2, "the second on its kept connection, then on a new one");
    }

    #[test]
    fn a_kept_connection_to_a_silent_mac_fails_as_a_new_one_would() {
        let (url, taken, hold) = keeping(false, true);
        let c = Client::at(vec![url], "t".into(), "m");
        c.new_round();
        c.post_json("/work/x", &serde_json::json!({})).unwrap();
        let e = c.post_json("/work/x", &serde_json::json!({})).unwrap_err();
        assert!(format!("{e:#}").contains("refused"), "{e:#}");
        assert!(c.round_failed(), "no answer: the round's later requests not tried");
        assert!(c.failed().is_some(), "unreachable, as a refused connection says");
        assert_eq!(taken.load(Ordering::SeqCst), 1);
        drop(hold);
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
