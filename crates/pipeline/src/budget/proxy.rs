//! The agent's proxy (docs/pool.md §7.7): every job's connections out go through it (`HTTPS_PROXY`
//! and the others set for each job, the job's sandbox denying any other), and those to a metered
//! host (crate::budget::class_of) take a token of the pool's budget before they're opened. A
//! tunnel (`CONNECT`, every HTTPS request's) is metered as one request: what's inside is the
//! client's TLS, end to end, never read (its certificate checks are the client's, unchanged). A
//! plain HTTP request (`GET http://…`) is passed on as one request, its connection closed after
//! it. It listens on this Mac alone (127.0.0.1). A process that sees a 429 or a `Retry-After`
//! tells it (`POST /hold?s=<seconds>&host=<host>`: crate::budget::seen), and the budget holds every
//! process and Mac; one that asks again tells it too (`POST /retry?host=<host>`: crate::budget::retried).
//! One fetching a host's pages tells its robots.txt (`POST /robots?host=<host>&state=<unchecked,
//! allowed or disallowed>[&t=<unix ms read>]`: crate::budget::net::Robots).
//! What goes through is counted per host as it's relayed, and per job by the credential its
//! clients send (`env_for`; crate::budget::net).

use super::net::{Host, Net, Outcome, Robots};
use super::{Budget, Class};
use anyhow::{Context, Result};
use std::io::{self, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpStream as Conn;

/// Where a host's connections go (the tests': a metered host answered by a server here); None:
/// looked up by name.
pub type Resolve = Arc<dyn Fn(&str, u16) -> Option<SocketAddr> + Send + Sync>;

/// The longest a request's head may be.
const HEAD_MAX: usize = 64 << 10;
/// How long a connection out may take to open.
const CONNECT_WAIT: Duration = Duration::from_secs(30);

/// The proxy's bounds: a client's head within `head`, a connection idle (nothing either way) for
/// `idle` ended (a query's tunnel charged as it ends), and at most `conns` connections out served at
/// once. One more waits its turn (first come, first served) for one of them to end, up to `wait`
/// (its client sees only a slow answer), and is answered 503 when it runs out; a hold told never
/// waits. At most twice `conns` clients at once in all: one more is answered 503 as it's taken.
/// (A terrain job keeps 64 tiles in flight, each its own tunnel: 512 serve eight such jobs at
/// once, and the turns share the Mac's connections out fairly among however many there are.)
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub head: Duration,
    pub idle: Duration,
    pub conns: usize,
    pub wait: Duration,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            head: Duration::from_secs(30),
            idle: Duration::from_secs(600),
            conns: 512,
            wait: Duration::from_secs(30),
        }
    }
}

/// The proxy's bounds' counts, for the tests (each host's: `Net`).
#[derive(Default)]
pub struct Counts {
    /// Connections that waited their turn (`Limits::conns` under way).
    pub waited: AtomicU64,
    /// Connections refused: too many clients, or a turn waited `Limits::wait` in vain.
    pub refused: AtomicU64,
}

/// What every connection shares.
struct Ctx {
    budget: Arc<Budget>,
    counts: Arc<Counts>,
    net: Arc<Net>,
    resolve: Option<Resolve>,
    limits: Limits,
    /// Clients, connections out, and the turns waiting.
    slots: std::sync::Mutex<Slots>,
    /// A hold being taken on the NAS (one at a time; a hold told meanwhile is held here at once).
    holding: AtomicBool,
    /// The thread taking it.
    hold_thread: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
}

/// The proxy, running.
pub struct Proxy {
    pub addr: SocketAddr,
    pub budget: Arc<Budget>,
    pub counts: Arc<Counts>,
    pub net: Arc<Net>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    ctx: Arc<Ctx>,
}

/// A listener for a proxy on 127.0.0.1:`port` (0: a port of its own): an error while another
/// socket listens there. No other socket may bind beside it (no `SO_REUSEPORT`: a job or another
/// process could take the jobs' connections), so the agent before stops its proxy before it lets
/// go of its lock, and the next binds the port its jobs were told as soon as it holds the lock.
pub fn listen(port: u16) -> io::Result<TcpListener> {
    use socket2::{Domain, Socket, Type};
    // (As store::spawn::listen: no test on a live program's port, #906.)
    assert!(
        !(store::spawn::POOL_PORTS.contains(&port) && store::spawn::in_test_program()),
        "a test's budgets proxy on {port}, a port of the live pool's (store::spawn::POOL_PORTS)"
    );
    store::spawn::sockets(|| {
        let s = Socket::new(Domain::IPV4, Type::STREAM, None)?;
        s.set_reuse_address(true)?;
        s.bind(&SocketAddr::from(([127, 0, 0, 1], port)).into())?;
        s.listen(1024)?;
        Ok(s.into())
    })
}

impl Proxy {
    /// Starts one on a port of its own on 127.0.0.1, taking from `budget`.
    pub fn start(budget: Arc<Budget>, resolve: Option<Resolve>) -> Result<Proxy> {
        Self::start_with(budget, resolve, Limits::default())
    }

    /// `start`, within `limits`.
    pub fn start_with(budget: Arc<Budget>, resolve: Option<Resolve>, limits: Limits) -> Result<Proxy> {
        Self::start_on(budget, resolve, limits, listen(0).context("the budgets' proxy: listen")?)
    }

    /// `start`, serving `listener` (`listen`'s).
    pub fn start_listening(budget: Arc<Budget>, resolve: Option<Resolve>, listener: TcpListener) -> Result<Proxy> {
        Self::start_on(budget, resolve, Limits::default(), listener)
    }

    fn start_on(budget: Arc<Budget>, resolve: Option<Resolve>, limits: Limits, listener: TcpListener) -> Result<Proxy> {
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let counts = Arc::new(Counts::default());
        let net = Arc::new(Net::default());
        let ctx = Arc::new(Ctx {
            budget: budget.clone(),
            counts: counts.clone(),
            net: net.clone(),
            resolve,
            limits,
            slots: Default::default(),
            holding: AtomicBool::new(false),
            hold_thread: Default::default(),
        });
        let (s, c) = (stop.clone(), ctx.clone());
        let thread = std::thread::Builder::new().name("budgets-proxy".into()).spawn(move || serve(listener, c, s))?;
        Ok(Proxy { addr, budget, counts, net, stop, thread: Some(thread), ctx })
    }

    /// Waits for the hold last told to be taken on the NAS (the tests'; the agent never waits).
    pub fn settle_hold(&self) {
        let t = self.ctx.hold_thread.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(t) = t {
            t.join().ok();
        }
    }

    /// The members the agent knows (itself among them): the NAS away, its share of the rate.
    pub fn set_members(&self, n: usize) {
        self.budget.members.store(n.max(1), Ordering::Relaxed);
    }

    /// What it has done, for the status: the requests to each class's hosts, and the holds told.
    pub fn view(&self) -> super::View {
        let held = self.budget.held_until();
        let asked = |c: Option<Class>| self.net.sum(|h| super::class_of(h) == c, |x| &x.requests);
        super::View {
            proxy: self.url(),
            wiki: asked(Some(Class::Wiki)),
            query: asked(Some(Class::Query)),
            other: asked(None),
            holds: self.net.sum(|_| true, |x| &x.holds),
            held_until_ms: (held > self.budget.now_ms()).then_some(held),
            away: self.budget.away(),
        }
    }

    /// Its counts of each host's traffic (the live store's).
    pub fn net(&self) -> Option<Arc<Net>> {
        Some(self.net.clone())
    }

    /// The port it listens on.
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Its address as a job is told it (`http://127.0.0.1:<port>`).
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The environment job `tag` is given: its connections through the proxy (each name a client
    /// may read), tagged (`http://<tag>:x@…`: its bytes counted as its, crate::budget::net::Net::job,
    /// from now on), but this Mac's own (the coordinator's, on 127.0.0.1), and where to tell a 429
    /// (untagged).
    pub fn env_for(&self, tag: &str) -> Vec<(String, String)> {
        let u = self.url();
        self.net.job_began(tag);
        let tagged = format!("http://{tag}:x@{}", self.addr);
        let mut v = Vec::new();
        for k in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"] {
            v.push((k.to_string(), tagged.clone()));
        }
        for k in ["NO_PROXY", "no_proxy"] {
            v.push((k.to_string(), "localhost,127.0.0.1,::1".to_string()));
        }
        v.push((super::PROXY_ENV.to_string(), u));
        v
    }

    /// Stops listening, once its thread has said so. Connections under way end as theirs do.
    pub fn stop(mut self) {
        self.halt();
    }

    fn halt(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            t.join().ok();
        }
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.halt();
    }
}

/// Waits until `fd` is readable or `ms` pass.
fn readable(fd: i32, ms: i32) -> bool {
    let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
    // SAFETY: poll on one descriptor the caller owns, its struct on the stack.
    unsafe { libc::poll(&mut p, 1, ms) > 0 }
}

/// The event loop's threads: every client and its connection out are served on them (task #908:
/// a thread per connection was 178 of the M4 agent's 265 threads with four jobs).
const IO_THREADS: usize = 2;
/// The most threads at once for what blocks (a token waited for, a host's name looked up, its
/// connection opened), started as needed and let go of after ten idle seconds.
const BLOCKING_THREADS: usize = 16;
/// The most connections of a class waiting for its token at once, each on one of those threads; the
/// rest wait their turn on the event loop (first come, first served), so a hold or a spent window
/// never takes the threads a host not metered needs to open its connections.
const TAKERS: usize = 4;

/// The process's proxies' event loop, never stopped (a proxy stopped leaves its connections under
/// way to end as theirs do).
fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(IO_THREADS)
            .max_blocking_threads(BLOCKING_THREADS)
            .thread_name("budgets-io")
            .enable_io()
            .enable_time()
            .build()
            .expect("the budgets' proxy: its event loop")
    })
}

/// The turns to wait for a token of `class` (`TAKERS` at once).
fn takers(class: Class) -> &'static tokio::sync::Semaphore {
    static T: [tokio::sync::Semaphore; 2] =
        [tokio::sync::Semaphore::const_new(TAKERS), tokio::sync::Semaphore::const_new(TAKERS)];
    &T[match class {
        Class::Wiki => 0,
        Class::Query => 1,
    }]
}

fn serve(listener: TcpListener, ctx: Arc<Ctx>, stop: Arc<AtomicBool>) {
    use std::os::fd::AsRawFd;
    let fd = listener.as_raw_fd();
    while !stop.load(Ordering::SeqCst) {
        // (The stop is looked at between waits: a fifth of a second.)
        if !readable(fd, 200) {
            continue;
        }
        match store::spawn::accept(&listener) {
            Ok((s, _)) => admit(s, &ctx),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => eprintln!("budgets' proxy: accept: {e}"),
        }
    }
}

/// The clients being served (twice `Limits::conns` at most), the connections out among them
/// (`Limits::conns` at most), and the turns waiting for one, in order.
#[derive(Default)]
struct Slots {
    clients: usize,
    live: usize,
    turns: std::collections::VecDeque<Arc<Turn>>,
}

/// A connection out waited for: given (`given`, under `Slots`' lock) the slot of one ending.
#[derive(Default)]
struct Turn {
    given: AtomicBool,
    told: tokio::sync::Notify,
}

fn slots(ctx: &Ctx) -> std::sync::MutexGuard<'_, Slots> {
    ctx.slots.lock().unwrap_or_else(|e| e.into_inner())
}

/// A client being served: let go of as it ends, however it ends.
struct Client(Arc<Ctx>);

impl Drop for Client {
    fn drop(&mut self) {
        slots(&self.0).clients -= 1;
    }
}

/// One of the `Limits::conns` connections out: as it ends, however it ends (a panic too), given to
/// the turn first in line, else freed.
struct Slot(Arc<Ctx>);

impl Drop for Slot {
    fn drop(&mut self) {
        let mut sl = slots(&self.0);
        match sl.turns.pop_front() {
            Some(t) => {
                t.given.store(true, Ordering::SeqCst);
                t.told.notify_one();
            }
            None => sl.live -= 1,
        }
    }
}

/// A connection out: at once while fewer than `conns` are open and none waits, else in turn, up
/// to `wait`; None when the wait runs out.
async fn slot(ctx: &Arc<Ctx>) -> Option<Slot> {
    let t = {
        let mut sl = slots(ctx);
        if sl.live < ctx.limits.conns && sl.turns.is_empty() {
            sl.live += 1;
            return Some(Slot(ctx.clone()));
        }
        ctx.counts.waited.fetch_add(1, Ordering::Relaxed);
        let t = Arc::new(Turn::default());
        sl.turns.push_back(t.clone());
        t
    };
    // (A slot given before this waits is kept by `told` till it does.)
    let _ = tokio::time::timeout(ctx.limits.wait, t.told.notified()).await;
    let mut sl = slots(ctx);
    if t.given.load(Ordering::SeqCst) {
        return Some(Slot(ctx.clone()));
    }
    sl.turns.retain(|x| !Arc::ptr_eq(x, &t));
    None
}

/// Answered 503 and closed (never waiting on the client).
fn refuse(s: &mut TcpStream, ctx: &Ctx) -> io::Result<()> {
    ctx.counts.refused.fetch_add(1, Ordering::Relaxed);
    s.set_nonblocking(true)?;
    write!(s, "{}", answer_text("503 Service Unavailable", TOO_MANY))?;
    s.shutdown(Shutdown::Write)
}

const TOO_MANY: &str = "too many connections through the proxy at once\n";

/// A client taken: served on the event loop while fewer than twice `conns` are.
fn admit(mut s: TcpStream, ctx: &Arc<Ctx>) {
    {
        let mut sl = slots(ctx);
        if sl.clients >= 2 * ctx.limits.conns {
            drop(sl);
            let _ = refuse(&mut s, ctx);
            return;
        }
        sl.clients += 1;
    }
    let client = Client(ctx.clone());
    runtime().spawn(async move {
        // (A client gone, a host that didn't answer: the client sees it.)
        let Ok(s) = s.set_nonblocking(true).and_then(|()| Conn::from_std(s)) else { return };
        let _ = handle(s, &client.0).await;
    });
}

/// A request's head: its first line's three parts, its header lines, and what came after it.
struct Head {
    method: String,
    target: String,
    version: String,
    headers: Vec<String>,
    rest: Vec<u8>,
}

/// Some bytes read from `s` into `buf`, waiting at most `idle` for them: an error when none came.
async fn read_some(s: &Conn, buf: &mut [u8], idle: Duration) -> io::Result<usize> {
    tokio::time::timeout(idle, async {
        loop {
            s.readable().await?;
            match s.try_read(buf) {
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => continue,
                r => return r,
            }
        }
    })
    .await
    .unwrap_or_else(|_| Err(io::Error::new(io::ErrorKind::TimedOut, "idle")))
}

/// `b` written to `s` whole, as fast as its reader takes it.
async fn write_all(s: &Conn, mut b: &[u8]) -> io::Result<()> {
    while !b.is_empty() {
        s.writable().await?;
        match s.try_write(b) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => b = &b[n..],
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn shutdown(s: &Conn, how: Shutdown) {
    let _ = socket2::SockRef::from(s).shutdown(how);
}

async fn read_head(s: &Conn, limit: Duration) -> io::Result<Head> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > HEAD_MAX {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "head too long"));
        }
        let n = read_some(s, &mut chunk, limit).await?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "closed before its head ended"));
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let text = String::from_utf8_lossy(&buf[..end]).into_owned();
    let mut lines = text.split("\r\n");
    let first = lines.next().unwrap_or_default();
    let mut parts = first.split_whitespace();
    let (method, target, version) = (
        parts.next().unwrap_or_default().to_string(),
        parts.next().unwrap_or_default().to_string(),
        parts.next().unwrap_or("HTTP/1.1").to_string(),
    );
    Ok(Head { method, target, version, headers: lines.map(str::to_string).collect(), rest: buf[end + 4..].to_vec() })
}

/// The job a request's head names by its credential (`Proxy-Authorization: Basic …`), if one
/// began with it.
fn job_of(head: &Head, net: &Net) -> Option<Arc<super::net::Job>> {
    head.headers.iter().find_map(|h| {
        let (name, v) = h.split_once(':')?;
        if !name.trim().eq_ignore_ascii_case("proxy-authorization") {
            return None;
        }
        let v = v.trim();
        v.get(..6).filter(|b| b.eq_ignore_ascii_case("basic "))?;
        net.job_of(v[6..].trim())
    })
}

fn answer_text(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

async fn answer(s: &Conn, status: &str, body: &str) -> io::Result<()> {
    write_all(s, answer_text(status, body).as_bytes()).await
}

/// `host:port` split, the port `default` when it's missing (an IPv6 address in brackets).
fn host_port(hp: &str, default: u16) -> Option<(String, u16)> {
    if let Some(rest) = hp.strip_prefix('[') {
        let (h, after) = rest.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(p) => p.parse().ok()?,
            None => default,
        };
        return Some((h.to_string(), port));
    }
    match hp.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => Some((h.to_string(), p.parse().ok()?)),
        _ => Some((hp.to_string(), default)),
    }
}

/// A connection to `host` opened (its name looked up first: both block, on a blocking thread).
async fn open(host: &str, port: u16, resolve: Option<&Resolve>) -> io::Result<Conn> {
    let (host, resolve) = (host.to_string(), resolve.cloned());
    let s = tokio::task::spawn_blocking(move || -> io::Result<TcpStream> {
        if let Some(a) = resolve.and_then(|r| r(&host, port)) {
            return TcpStream::connect_timeout(&a, CONNECT_WAIT);
        }
        let mut last = io::Error::new(io::ErrorKind::NotFound, format!("{host}: no address"));
        for a in (host.as_str(), port).to_socket_addrs()? {
            match TcpStream::connect_timeout(&a, CONNECT_WAIT) {
                Ok(s) => return Ok(s),
                Err(e) => last = e,
            }
        }
        Err(last)
    })
    .await
    .map_err(io::Error::other)??;
    s.set_nonblocking(true)?;
    Conn::from_std(s)
}

/// A token for `host`'s connection, when it's metered (its class: waited for in turn, `TAKERS`
/// at once), and its request counted.
async fn meter(host: &str, budget: &Arc<Budget>, net: &Net) -> io::Result<(Option<Class>, Arc<Host>)> {
    let class = super::class_of(host);
    if let Some(c) = class {
        let _turn = takers(c).acquire().await.map_err(io::Error::other)?;
        let b = budget.clone();
        tokio::task::spawn_blocking(move || b.take(c)).await.map_err(io::Error::other)?;
    }
    let h = net.host(host);
    h.requests.fetch_add(1, Ordering::Relaxed);
    Ok((class, h))
}

/// A connection out to `host` for its client `s`: opened (counted, and open until its `Open` is
/// dropped), else answered 502 (an event).
async fn open_out(s: &Conn, ctx: &Ctx, host: &str, port: u16, h: &Arc<Host>, t0: Instant) -> Option<(Conn, Open)> {
    match open(host, port, ctx.resolve.as_ref()).await {
        Ok(u) => {
            h.conns.fetch_add(1, Ordering::Relaxed);
            h.open.fetch_add(1, Ordering::Relaxed);
            Some((u, Open(h.clone())))
        }
        Err(e) => {
            ctx.net.ended(host, (0, 0), t0.elapsed().as_millis() as u64, Outcome::Unreached);
            let _ = answer(s, "502 Bad Gateway", &format!("{host}:{port}: {e}\n")).await;
            None
        }
    }
}

/// A host's connection out open: its count of those open taken back as it's dropped (however the
/// connection ends).
struct Open(Arc<Host>);

impl Drop for Open {
    fn drop(&mut self) {
        self.0.open.fetch_sub(1, Ordering::Relaxed);
    }
}

/// No connection out for `host`'s client: answered 503, an event.
async fn refuse_for(s: &Conn, ctx: &Ctx, host: &str) -> io::Result<()> {
    ctx.net.ended(host, (0, 0), 0, Outcome::Refused);
    ctx.counts.refused.fetch_add(1, Ordering::Relaxed);
    answer(s, "503 Service Unavailable", TOO_MANY).await?;
    shutdown(s, Shutdown::Write);
    Ok(())
}

/// A query string's value of `key`.
fn param<'a>(q: &'a str, key: &str) -> Option<&'a str> {
    q.trim_start_matches('?').split('&').find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
}

/// A transfer relayed: its event, of its bytes `b` (in, out) and the `sent` with its head.
fn ended(net: &Net, host: &str, b: (u64, u64), sent: u64, t0: Instant, ok: bool) {
    let outcome = if ok { Outcome::Ok } else { Outcome::Broken };
    net.ended(host, (b.0, b.1 + sent), t0.elapsed().as_millis() as u64, outcome);
}

async fn handle(s: Conn, ctx: &Arc<Ctx>) -> io::Result<()> {
    let (budget, net) = (&ctx.budget, &*ctx.net);
    let head = read_head(&s, ctx.limits.head).await?;
    // (From here on a connection idle `limits.idle` ends.)
    if head.method.eq_ignore_ascii_case("CONNECT") {
        let Some((host, port)) = host_port(&head.target, 443) else {
            return answer(&s, "400 Bad Request", "CONNECT host:port\n").await;
        };
        let Some(_slot) = slot(ctx).await else { return refuse_for(&s, ctx, &host).await };
        let (class, h) = meter(&host, budget, net).await?;
        let (began, t0) = (budget.now_ms(), Instant::now());
        let Some((up, _open)) = open_out(&s, ctx, &host, port, &h, t0).await else { return Ok(()) };
        // (HTTP/1.0: macOS's nc, the ssh tunnel's ProxyCommand (osmpass::streamed), takes no other.)
        write_all(&s, b"HTTP/1.0 200 Connection established\r\n\r\n").await?;
        write_all(&up, &head.rest).await?;
        let sent = head.rest.len() as u64;
        h.bytes_out.fetch_add(sent, Ordering::Relaxed);
        let job = job_of(&head, net);
        if let Some(j) = &job {
            j.bytes_out.fetch_add(sent, Ordering::Relaxed);
        }
        let done = || charge(budget, class, began);
        relay(&s, &up, &h, job, ctx.limits.idle, done, |b, ok| ended(net, &host, b, sent, t0, ok)).await;
        return Ok(());
    }
    // (The proxy's own: a 429 told, a retry told.)
    if head.target.starts_with('/') {
        if let Some(q) = head.target.strip_prefix("/retry") {
            net.host(param(q, "host").unwrap_or("?")).retries.fetch_add(1, Ordering::Relaxed);
            return answer(&s, "200 OK", "counted\n").await;
        }
        if let Some(q) = head.target.strip_prefix("/robots") {
            let (Some(host), Some(state)) = (param(q, "host"), param(q, "state").and_then(Robots::parse)) else {
                return answer(&s, "400 Bad Request", "/robots?host=<host>&state=<unchecked, allowed or disallowed>\n")
                    .await;
            };
            let t = param(q, "t").and_then(|v| v.parse::<u64>().ok());
            if !net.robots(host, state, t) {
                return answer(&s, "400 Bad Request", "a host's name (and room for it)\n").await;
            }
            return answer(&s, "200 OK", "told\n").await;
        }
        if let Some(q) = head.target.strip_prefix("/hold") {
            let secs = param(q, "s")
                .and_then(|v| v.parse::<f64>().ok())
                .map_or(super::HOLD_DEFAULT_S, |v| v.ceil().max(1.0) as u64);
            net.host(param(q, "host").unwrap_or("?")).holds.fetch_add(1, Ordering::Relaxed);
            // (Held here at once; on the NAS on a thread of its own, one at a time, so the teller
            // isn't kept waiting on the share: told is told.)
            let (now, until) = budget.hold_here(secs);
            if !ctx.holding.swap(true, Ordering::SeqCst) {
                let c = ctx.clone();
                let spawned = std::thread::Builder::new().name("budgets-hold".into()).spawn(move || {
                    c.budget.hold_nas(now, until);
                    c.holding.store(false, Ordering::SeqCst);
                });
                match spawned {
                    Ok(t) => *ctx.hold_thread.lock().unwrap_or_else(|e| e.into_inner()) = Some(t),
                    Err(_) => ctx.holding.store(false, Ordering::SeqCst),
                }
            }
            return answer(&s, "200 OK", "held\n").await;
        }
        return answer(&s, "404 Not Found", "a proxy: CONNECT, an absolute URL, /hold, /retry or /robots\n").await;
    }
    // A plain HTTP request by its absolute URL: passed on, its connection closed after it.
    let Some(after) = head.target.strip_prefix("http://") else {
        return answer(&s, "400 Bad Request", "only http:// URLs, or CONNECT\n").await;
    };
    let (hp, path) = match after.find('/') {
        Some(i) => (&after[..i], &after[i..]),
        None => (after, "/"),
    };
    let Some((host, port)) = host_port(hp, 80) else { return answer(&s, "400 Bad Request", "bad host\n").await };
    let Some(_slot) = slot(ctx).await else { return refuse_for(&s, ctx, &host).await };
    let (class, h) = meter(&host, budget, net).await?;
    let (began, t0) = (budget.now_ms(), Instant::now());
    let Some((up, _open)) = open_out(&s, ctx, &host, port, &h, t0).await else { return Ok(()) };
    let mut out = format!("{} {path} {}\r\n", head.method, head.version);
    for h in &head.headers {
        let name = h.split(':').next().unwrap_or_default().trim().to_ascii_lowercase();
        if matches!(name.as_str(), "connection" | "proxy-connection" | "keep-alive" | "proxy-authorization") {
            continue;
        }
        out.push_str(h);
        out.push_str("\r\n");
    }
    out.push_str("Connection: close\r\n\r\n");
    write_all(&up, out.as_bytes()).await?;
    write_all(&up, &head.rest).await?;
    let sent = (out.len() + head.rest.len()) as u64;
    h.bytes_out.fetch_add(sent, Ordering::Relaxed);
    let job = job_of(&head, net);
    if let Some(j) = &job {
        j.bytes_out.fetch_add(sent, Ordering::Relaxed);
    }
    let done = || charge(budget, class, began);
    relay(&s, &up, &h, job, ctx.limits.idle, done, |b, ok| ended(net, &host, b, sent, t0, ok)).await;
    Ok(())
}

/// A metered connection begun at `began` (the budget's clock) has ended: its time charged.
fn charge(budget: &Budget, class: Option<Class>, began: u64) {
    if let Some(c) = class {
        budget.charge(c, budget.now_ms().saturating_sub(began));
    }
}

/// Copies `r` to `w` until `r` ends (or `stop` is told), each chunk's bytes added to `n` (and its
/// job's `j`) as it's written, a read waiting at most `idle`: the bytes copied, and whether it ended
/// with no error. A write waits for `w`'s reader as long as it takes (the back-pressure).
async fn copy_counted(
    r: &Conn,
    w: &Conn,
    n: &AtomicU64,
    j: Option<&AtomicU64>,
    idle: Duration,
    stop: &tokio::sync::Notify,
) -> (u64, bool) {
    let mut buf = vec![0u8; 64 << 10];
    let mut all = 0u64;
    loop {
        let k = tokio::select! {
            r = read_some(r, &mut buf, idle) => match r {
                Ok(0) => return (all, true),
                Ok(k) => k,
                Err(_) => return (all, false),
            },
            () = stop.notified() => return (all, true),
        };
        if write_all(w, &buf[..k]).await.is_err() {
            return (all, false);
        }
        all += k as u64;
        n.fetch_add(k as u64, Ordering::Relaxed);
        if let Some(j) = j {
            j.fetch_add(k as u64, Ordering::Relaxed);
        }
    }
}

/// Copies each way until both have ended (a way idle `idle` ends: the client's told to the host as
/// its end; the host's ends the whole connection), each way's bytes counted to `h` (and its `job`'s)
/// as they go; `done` run before the client's side is closed (what it does is done before the
/// client sees its answer end), then `end` told the bytes (in, out) and whether neither way broke.
async fn relay(
    client: &Conn,
    up: &Conn,
    h: &Arc<Host>,
    job: Option<Arc<super::net::Job>>,
    idle: Duration,
    done: impl FnOnce(),
    end: impl FnOnce((u64, u64), bool),
) {
    let (stop, never) = (tokio::sync::Notify::new(), tokio::sync::Notify::new());
    let to_host = async {
        // (The client's side ended, idle or broken: told to the host as its end, whose answer still
        // comes back; a host that then stays idle too is ended by its own read's wait.)
        let r = copy_counted(client, up, &h.bytes_out, job.as_ref().map(|j| &j.bytes_out), idle, &stop).await;
        shutdown(up, Shutdown::Write);
        r
    };
    let to_client = async {
        let (got, ok) = copy_counted(up, client, &h.bytes_in, job.as_ref().map(|j| &j.bytes_in), idle, &never).await;
        if !ok {
            shutdown(up, Shutdown::Both);
            stop.notify_one();
        }
        done();
        shutdown(client, Shutdown::Write);
        (got, ok)
    };
    let ((sent, sent_ok), (got, ok)) = tokio::join!(to_host, to_client);
    end((got, sent), ok && sent_ok);
}

#[cfg(test)]
mod tests {
    use super::super::{Clock, Source, StepClock, Token};
    use super::*;
    use std::collections::BTreeMap;
    use std::io::Read;

    const T0: u64 = 1_800_000_000_000;

    /// A server here standing for every host: it echoes what it's sent until the client's side
    /// ends (`then`, run as each connection's first bytes arrive, before they're echoed); a plain
    /// HTTP request's head is kept (`heads`).
    struct Upstream {
        addr: SocketAddr,
        heads: Arc<std::sync::Mutex<Vec<String>>>,
    }

    fn upstream(then: Arc<dyn Fn() + Send + Sync>) -> Upstream {
        let l = store::spawn::listen("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let addr = l.local_addr().unwrap();
        let heads = Arc::new(std::sync::Mutex::new(Vec::new()));
        let h = heads.clone();
        std::thread::spawn(move || {
            use std::os::fd::AsRawFd;
            loop {
                if !readable(l.as_raw_fd(), -1) {
                    continue;
                }
                let Ok((mut s, _)) = store::spawn::accept(&l) else { continue };
                let (then, h) = (then.clone(), h.clone());
                std::thread::spawn(move || {
                    s.set_nonblocking(false).unwrap();
                    let mut all = Vec::new();
                    let mut b = [0u8; 4096];
                    let mut first = true;
                    loop {
                        let n = s.read(&mut b).unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        if first {
                            then();
                            first = false;
                        }
                        all.extend_from_slice(&b[..n]);
                        s.write_all(&b[..n]).unwrap();
                    }
                    if all.starts_with(b"GET ") {
                        h.lock().unwrap().push(String::from_utf8_lossy(&all).into_owned());
                    }
                });
            }
        });
        Upstream { addr, heads }
    }

    fn proxy(root: &store::nasfs::NasRoot, me: &str, clock: &Arc<StepClock>, up: &Upstream) -> Proxy {
        let b = Arc::new(Budget::new(root, me, clock.clone()).with_ledger());
        let a = up.addr;
        Proxy::start(b, Some(Arc::new(move |_: &str, _: u16| Some(a)))).unwrap()
    }

    /// A tunnel to `host` through `p`: `msg` sent, the client's side ended, what came back.
    fn tunnel(p: &SocketAddr, host: &str, msg: &[u8]) -> Vec<u8> {
        let mut s = TcpStream::connect(p).unwrap();
        write!(s, "CONNECT {host}:443 HTTP/1.1\r\nHost: {host}:443\r\n\r\n").unwrap();
        let mut head = Vec::new();
        let mut b = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            assert_eq!(s.read(&mut b).unwrap(), 1, "the proxy closed: {}", String::from_utf8_lossy(&head));
            head.push(b[0]);
        }
        assert!(head.starts_with(b"HTTP/1.0 200"), "{}", String::from_utf8_lossy(&head));
        s.write_all(msg).unwrap();
        s.shutdown(Shutdown::Write).unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        out
    }

    fn request(p: &SocketAddr, req: &str) -> String {
        let mut s = TcpStream::connect(p).unwrap();
        s.write_all(req.as_bytes()).unwrap();
        s.shutdown(Shutdown::Write).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    fn per_window(ledgers: &[Vec<Token>]) -> BTreeMap<(Class, u64), u32> {
        let mut m = BTreeMap::new();
        for t in ledgers.iter().flatten() {
            *m.entry((t.class, t.window)).or_default() += 1;
        }
        m
    }

    /// Two agents' proxies on one NAS folder, clients hammering both: every tunnel opened, the
    /// bytes through unchanged, and no window past its tokens across the two.
    #[test]
    fn two_agents_share_one_budget_never_past_its_rate() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(StepClock::at(T0));
        let up = upstream(Arc::new(|| {}));
        let (a, b) = (
            proxy(&store::nasfs::NasRoot::local_for_test(dir.path()), "m-a", &clock, &up),
            proxy(&store::nasfs::NasRoot::local_for_test(dir.path()), "m-b", &clock, &up),
        );
        let mut ts = Vec::new();
        for i in 0..6 {
            let p = if i % 2 == 0 { a.addr } else { b.addr };
            ts.push(std::thread::spawn(move || {
                for k in 0..20 {
                    let host = if (i + k) % 3 == 0 { "query.wikidata.org" } else { "en.wikipedia.org" };
                    let msg = format!("hello {i} {k}\n");
                    assert_eq!(tunnel(&p, host, msg.as_bytes()), msg.as_bytes());
                }
            }));
        }
        ts.into_iter().for_each(|t| t.join().unwrap());
        let w = per_window(&[a.budget.ledger(), b.budget.ledger()]);
        assert_eq!(w.values().sum::<u32>(), 120);
        for ((class, win), n) in &w {
            assert!(*n <= class.rate().tokens, "{class:?} window {win}: {n}");
        }
        let n = |p: &Proxy| p.view().wiki + p.view().query;
        assert_eq!(n(&a) + n(&b), 120);
        assert_eq!(a.view().other + b.view().other, 0);
    }

    /// A 429 told to one agent's proxy holds the other's connections till its wait ends.
    #[test]
    fn a_429_told_to_one_proxy_holds_the_other() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(StepClock::at(T0));
        let up = upstream(Arc::new(|| {}));
        let (a, b) = (
            proxy(&store::nasfs::NasRoot::local_for_test(dir.path()), "m-a", &clock, &up),
            proxy(&store::nasfs::NasRoot::local_for_test(dir.path()), "m-b", &clock, &up),
        );
        let r = request(&a.addr, "POST /hold?s=40 HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n");
        assert!(r.starts_with("HTTP/1.1 200") && r.ends_with("held\n"), "{r}");
        // (Its proxy held at once; the NAS's part on a thread: waited for here.)
        assert!(a.budget.held_until() >= T0 + 40_000);
        a.settle_hold();
        assert_eq!(tunnel(&b.addr, "qlever.dev", b"q\n"), b"q\n");
        let t = b.budget.ledger()[0];
        assert!(t.window >= (T0 + 40_000) / 10_000, "{t:?}");
        assert_eq!(t.source, Source::Nas);
        assert!(request(&a.addr, "GET /other HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 404"));
    }

    /// Tasks #393, #588: a proxy's port can't be bound while a proxy or any other socket listens
    /// there (none beside it), and can be again once it has stopped.
    #[test]
    fn a_proxys_port_is_bound_by_none_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let first = Proxy::start(
            Arc::new(Budget::new(
                &store::nasfs::NasRoot::local_for_test(dir.path()),
                "m-a",
                Arc::new(StepClock::at(T0)),
            )),
            None,
        )
        .unwrap();
        let port = first.port();
        assert!(listen(port).is_err());
        first.stop();
        assert!(listen(port).is_ok());
        let plain = store::spawn::listen("127.0.0.1:0").unwrap();
        assert!(listen(plain.local_addr().unwrap().port()).is_err());
    }

    fn proxy_within(root: &store::nasfs::NasRoot, clock: &Arc<StepClock>, up: &Upstream, limits: Limits) -> Proxy {
        let b = Arc::new(Budget::new(root, "m-a", clock.clone()).with_ledger());
        let a = up.addr;
        Proxy::start_with(b, Some(Arc::new(move |_: &str, _: u16| Some(a))), limits).unwrap()
    }

    /// The bounds, each with only its own limit short (the others a minute): a client that sends
    /// nothing is closed with no answer; a query's tunnel whose bytes all came with its head (so
    /// nothing the test does is timed), then left idle by the client, is ended once the host has
    /// answered, and charged (seven seconds, on the budget's clock, moved by the host as it
    /// answers); one connection past the most at once, given no wait, is answered 503. No assert depends on when
    /// the test's own thread runs.
    #[test]
    fn a_silent_client_an_idle_tunnel_and_too_many_connections_are_ended() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(StepClock::at(T0));
        let c = clock.clone();
        let up = upstream(Arc::new(move || c.set(c.now_ms() + 7_000)));
        let (short, long) = (Duration::from_millis(100), Duration::from_secs(60));
        // (Nothing sent: closed at its head's limit, nothing answered.)
        let h = proxy_within(
            &store::nasfs::NasRoot::local_for_test(&dir.path().join("h")),
            &clock,
            &up,
            Limits { head: short, idle: long, conns: 8, wait: long },
        );
        let mut out = Vec::new();
        TcpStream::connect(h.addr).unwrap().read_to_end(&mut out).unwrap();
        assert!(out.is_empty(), "{}", String::from_utf8_lossy(&out));
        // (A query's tunnel, its byte sent with its head; the client never ends its side.)
        let a = proxy_within(
            &store::nasfs::NasRoot::local_for_test(dir.path()),
            &clock,
            &up,
            Limits { head: long, idle: short, conns: 8, wait: long },
        );
        let mut s = TcpStream::connect(a.addr).unwrap();
        s.write_all(b"CONNECT query.wikidata.org:443 HTTP/1.1\r\n\r\nq").unwrap();
        let mut all = Vec::new();
        s.read_to_end(&mut all).unwrap();
        assert_eq!(all, b"HTTP/1.0 200 Connection established\r\n\r\nq", "{}", String::from_utf8_lossy(&all));
        // (Charged as it ended, before the client saw its end: four tokens of the window's five
        // spent with its own, so the next query takes the fifth, leasing every slot, or the next
        // window's.)
        let w = a.budget.ledger()[0].window;
        assert_eq!(tunnel(&a.addr, "query.wikidata.org", b"x"), b"x");
        let next = a.budget.ledger()[1].window;
        let leased = (0..5).filter(|s| a.budget.dir(Class::Query).join(format!("{w}-{s}")).is_file()).count();
        assert!(next > w || leased == 5, "charged: {leased} slots of window {w}");
        // (One connection out at most, no wait: while one is open, the next is answered 503; a
        // hold told meanwhile isn't kept waiting.)
        let b = proxy_within(
            &store::nasfs::NasRoot::local_for_test(&dir.path().join("b")),
            &clock,
            &up,
            Limits { head: long, idle: long, conns: 1, wait: Duration::ZERO },
        );
        let mut open = TcpStream::connect(b.addr).unwrap();
        write!(open, "CONNECT s3.amazonaws.com:443 HTTP/1.1\r\n\r\n").unwrap();
        let mut head = [0u8; 39];
        open.read_exact(&mut head).unwrap();
        let r = request(&b.addr, "CONNECT s3.amazonaws.com:443 HTTP/1.1\r\n\r\n");
        assert!(r.starts_with("HTTP/1.1 503"), "{r}");
        assert_eq!(b.counts.refused.load(Ordering::Relaxed), 1);
        let r = request(&b.addr, "POST /hold?s=5 HTTP/1.1\r\n\r\n");
        assert!(r.ends_with("held\n"), "{r}");
        b.settle_hold();
        drop(open);
    }

    /// Task #602: twice as many clients at once as the proxy serves (each tunnel held open a
    /// moment by the host): none refused, every tunnel's bytes through, the budget taken once each.
    #[test]
    fn more_clients_than_connections_all_wait_their_turn() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(StepClock::at(T0));
        let up = upstream(Arc::new(|| std::thread::sleep(Duration::from_millis(50))));
        let long = Duration::from_secs(60);
        let a = proxy_within(
            &store::nasfs::NasRoot::local_for_test(dir.path()),
            &clock,
            &up,
            Limits { head: long, idle: long, conns: 4, wait: long },
        );
        let p = a.addr;
        let ts: Vec<_> = (0..8)
            .map(|i| {
                std::thread::spawn(move || {
                    let host = if i % 2 == 0 { "en.wikipedia.org" } else { "s3.amazonaws.com" };
                    let msg = format!("tile {i}\n");
                    assert_eq!(tunnel(&p, host, msg.as_bytes()), msg.as_bytes());
                })
            })
            .collect();
        ts.into_iter().for_each(|t| t.join().unwrap());
        assert_eq!(a.counts.refused.load(Ordering::Relaxed), 0);
        assert!(a.counts.waited.load(Ordering::Relaxed) > 0, "the cap was never reached");
        assert_eq!(a.view().wiki, 4);
        assert_eq!(a.view().other, 4);
        assert_eq!(a.budget.ledger().len(), 4);
    }

    /// Task #518: a job's tagged requests (`env_for`'s credential) counted as its, beside its host's;
    /// an untagged one, and one tagged for no job begun, to its host alone.
    #[test]
    fn a_jobs_tagged_traffic_is_counted_as_its() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(StepClock::at(T0));
        let up = upstream(Arc::new(|| {}));
        let a = up.addr;
        let b = Arc::new(
            Budget::new(&store::nasfs::NasRoot::local_for_test(dir.path()), "m-a", clock.clone()).with_ledger(),
        );
        let p = Proxy::start(b, Some(Arc::new(move |_: &str, _: u16| Some(a)))).unwrap();
        let env = p.env_for("job2");
        assert!(env.contains(&("HTTPS_PROXY".into(), format!("http://job2:x@{}", p.addr))));
        assert!(env.contains(&(super::super::PROXY_ENV.into(), p.url())));
        let req = |auth: &str| {
            request(&p.addr, &format!("GET http://s3.amazonaws.com/x HTTP/1.1\r\nHost: s3.amazonaws.com\r\n{auth}\r\n"))
        };
        let tagged = req(&format!("Proxy-Authorization: Basic {}\r\n", super::super::net::credential("job2")));
        req("");
        req(&format!("proxy-authorization: basic {}\r\n", super::super::net::credential("job9")));
        let (got, sent) = p.net.job("job2").unwrap();
        assert_eq!(got, tagged.len() as u64);
        assert!(sent > 0 && sent < 200, "{sent}");
        assert!(p.net.view()["hosts"]["s3.amazonaws.com"]["in"].as_u64().unwrap() > got * 2);
        assert!(p.net.job("job9").is_none());
    }

    /// Task #516: each host's bytes, requests and connections as they're relayed (a tunnel's, a
    /// plain request's), a retry and a 429 told counted for their host, and each transfer an event:
    /// one to a host that can't be reached among them. A host's robots.txt told is kept (task #635).
    #[test]
    fn each_hosts_traffic_retries_holds_and_robots_are_counted() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(StepClock::at(T0));
        let up = upstream(Arc::new(|| {}));
        let a = up.addr;
        let b = Arc::new(
            Budget::new(&store::nasfs::NasRoot::local_for_test(dir.path()), "m-a", clock.clone()).with_ledger(),
        );
        // (gone.example: a port nothing listens on.)
        let gone = store::spawn::listen("127.0.0.1:0").unwrap().local_addr().unwrap();
        let p =
            Proxy::start(b, Some(Arc::new(move |h: &str, _: u16| Some(if h == "gone.example" { gone } else { a }))))
                .unwrap();
        let msg = vec![7u8; 300_000];
        assert_eq!(tunnel(&p.addr, "S3.amazonaws.com", &msg), msg);
        let req = "GET http://dumps.wikimedia.org/x HTTP/1.1\r\nHost: dumps.wikimedia.org\r\n\r\n";
        let echoed = request(&p.addr, req);
        assert!(request(&p.addr, "POST /retry?host=dumps.wikimedia.org HTTP/1.1\r\n\r\n").ends_with("counted\n"));
        assert!(request(&p.addr, "POST /robots?host=www.historicplaces.ca&state=allowed HTTP/1.1\r\n\r\n")
            .ends_with("told\n"));
        assert!(request(&p.addr, "POST /robots?host=x&state=maybe HTTP/1.1\r\n\r\n").contains("400"));
        assert!(request(&p.addr, "POST /robots?host=a%2Fb&state=allowed HTTP/1.1\r\n\r\n").contains("400"));
        let r = request(&p.addr, "POST /hold?s=5&host=dumps.wikimedia.org HTTP/1.1\r\n\r\n");
        assert!(r.ends_with("held\n"), "{r}");
        p.settle_hold();
        let r = request(&p.addr, "CONNECT gone.example:443 HTTP/1.1\r\n\r\n");
        assert!(r.starts_with("HTTP/1.1 502"), "{r}");
        let v = p.net.view();
        let s3 = &v["hosts"]["s3.amazonaws.com"];
        assert_eq!((s3["in"].as_u64(), s3["out"].as_u64()), (Some(300_000), Some(300_000)), "{v}");
        assert_eq!((s3["requests"].as_u64(), s3["conns"].as_u64(), s3["open"].as_u64()), (Some(1), Some(1), Some(0)));
        let d = &v["hosts"]["dumps.wikimedia.org"];
        // (Its head as passed on, echoed back.)
        assert_eq!(d["in"].as_u64(), Some(echoed.len() as u64), "{v}");
        assert_eq!(d["out"], d["in"]);
        assert_eq!((d["retries"].as_u64(), d["holds"].as_u64()), (Some(1), Some(1)));
        assert_eq!(d["robots"], serde_json::Value::Null);
        assert_eq!(v["hosts"]["www.historicplaces.ca"]["robots"]["state"], "allowed");
        let g = &v["hosts"]["gone.example"];
        assert_eq!((g["requests"].as_u64(), g["conns"].as_u64()), (Some(1), Some(0)));
        let ev: Vec<_> =
            v["events"].as_array().unwrap().iter().map(|e| (e["host"].clone(), e["outcome"].clone())).collect();
        assert_eq!(
            ev,
            [("s3.amazonaws.com", "ok"), ("dumps.wikimedia.org", "ok"), ("gone.example", "unreached")]
                .map(|(h, o)| (serde_json::json!(h), serde_json::json!(o)))
        );
        assert_eq!(v["events"][0]["in"], 300_000);
        let s = p.view();
        assert_eq!((s.wiki, s.other, s.holds), (1, 2, 1));
    }

    /// A hold told to a proxy whose NAS is away is held there at once all the same (told is told).
    #[test]
    fn a_hold_is_held_before_the_nas_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(StepClock::at(T0));
        let up = upstream(Arc::new(|| {}));
        let a = proxy(&store::nasfs::NasRoot::local_for_test(&dir.path().join("away")), "m-a", &clock, &up);
        let r = request(&a.addr, "POST /hold?s=30 HTTP/1.1\r\n\r\n");
        assert!(r.ends_with("held\n"), "{r}");
        assert_eq!(a.budget.held_until(), T0 + 30_000);
        a.settle_hold();
        assert!(!dir.path().join("away").exists());
    }

    /// A query's tunnel open for seven seconds owes three tokens past its own, paid before the next.
    #[test]
    fn a_tunnels_time_is_charged_to_the_query_services() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(StepClock::at(T0));
        let c = clock.clone();
        let up = upstream(Arc::new(move || c.set(c.now_ms() + 7_000)));
        let a = proxy(&store::nasfs::NasRoot::local_for_test(dir.path()), "m-a", &clock, &up);
        assert_eq!(tunnel(&a.addr, "query.wikidata.org", b"q\n"), b"q\n");
        let w0 = a.budget.ledger()[0].window;
        assert_eq!(tunnel(&a.addr, "en.wikipedia.org", b"x"), b"x", "another class owes nothing");
        assert_eq!(tunnel(&a.addr, "query.wikidata.org", b"q\n"), b"q\n");
        let qs: Vec<_> = a.budget.ledger().into_iter().filter(|t| t.class == Class::Query).collect();
        let w1 = qs[1].window;
        let leased = |w: u64| (0..5).filter(|s| a.budget.dir(Class::Query).join(format!("{w}-{s}")).is_file()).count();
        assert_eq!(leased(w0) + if w1 != w0 { leased(w1) } else { 0 }, 5, "one, three owed, one");
    }

    /// A plain HTTP request is passed on by its path, its connection closed after it; a host not
    /// metered takes nothing.
    #[test]
    fn plain_http_is_passed_on_unmetered_but_for_wikimedia() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(StepClock::at(T0));
        let up = upstream(Arc::new(|| {}));
        let a = proxy(&store::nasfs::NasRoot::local_for_test(dir.path()), "m-a", &clock, &up);
        let r = request(&a.addr, "GET http://example.test:8080/a/b?c=1 HTTP/1.1\r\nHost: example.test:8080\r\nProxy-Connection: keep-alive\r\n\r\n");
        assert!(r.starts_with("GET /a/b?c=1 HTTP/1.1\r\n"), "{r}");
        assert!(r.contains("Connection: close\r\n") && !r.contains("Proxy-Connection"), "{r}");
        assert_eq!(a.view().other, 1);
        assert!(a.budget.ledger().is_empty());
        let r = request(&a.addr, "GET http://dumps.wikimedia.org/x HTTP/1.1\r\nHost: dumps.wikimedia.org\r\n\r\n");
        assert!(r.starts_with("GET /x HTTP/1.1"), "{r}");
        assert_eq!(a.budget.ledger().len(), 1);
        assert_eq!(up.heads.lock().unwrap().len(), 2);
        assert_eq!(tunnel(&a.addr, "s3.amazonaws.com", b"z"), b"z");
        assert_eq!(a.budget.ledger().len(), 1);
        assert_eq!(a.view().other, 2);
    }

    /// This process's threads and resident bytes (the kernel's count).
    fn threads_and_rss() -> (i32, u64) {
        // SAFETY: a plain C struct, zeroes valid.
        let mut ti: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_taskinfo>() as i32;
        // SAFETY: the struct is ours, its size given.
        let n = unsafe {
            libc::proc_pidinfo(
                std::process::id() as i32,
                libc::PROC_PIDTASKINFO,
                0,
                (&mut ti as *mut libc::proc_taskinfo).cast(),
                size,
            )
        };
        assert_eq!(n, size, "{}", io::Error::last_os_error());
        (ti.pti_threadnum, ti.pti_resident_size)
    }

    /// A host here that echoes each connection until its client ends it, on one thread of its
    /// own (an event loop), so the threads counted are the proxy's.
    fn upstream_one_thread() -> SocketAddr {
        let l = store::spawn::listen("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_io().build().unwrap();
            rt.block_on(async move {
                let l = tokio::net::TcpListener::from_std(l).unwrap();
                loop {
                    let Ok((mut s, _)) = l.accept().await else { continue };
                    tokio::spawn(async move {
                        let (mut r, mut w) = s.split();
                        let _ = tokio::io::copy(&mut r, &mut w).await;
                    });
                }
            });
        });
        addr
    }

    /// Task #908: 256 tunnels open at once through two agents' proxies on one budget (eight
    /// terrain-like jobs' worth, a third of them metered) are served by a bounded number of
    /// threads; every tunnel's bytes come through; no window is past its tokens.
    #[test]
    fn many_connections_take_few_threads_and_the_budget_holds() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(StepClock::at(T0));
        let up = upstream_one_thread();
        let mk = |me: &str| {
            let root = store::nasfs::NasRoot::local_for_test(dir.path());
            let b = Arc::new(Budget::new(&root, me, clock.clone()).with_ledger());
            Proxy::start(b, Some(Arc::new(move |_: &str, _: u16| Some(up)))).unwrap()
        };
        let (a, b) = (mk("m-a"), mk("m-b"));
        let (t0, m0) = threads_and_rss();
        let n = 256;
        let mut open = Vec::new();
        for i in 0..n {
            let p = if i % 2 == 0 { a.addr } else { b.addr };
            let host = match i % 6 {
                0 => "query.wikidata.org",
                1 => "en.wikipedia.org",
                _ => "s3.amazonaws.com",
            };
            let mut s = TcpStream::connect(p).unwrap();
            write!(s, "CONNECT {host}:443 HTTP/1.1\r\n\r\n").unwrap();
            open.push(s);
        }
        for (i, s) in open.iter_mut().enumerate() {
            let mut head = [0u8; 39];
            s.read_exact(&mut head).unwrap();
            assert!(head.starts_with(b"HTTP/1.0 200"), "{i}: {}", String::from_utf8_lossy(&head));
            writeln!(s, "tile {i}").unwrap();
        }
        for (i, s) in open.iter_mut().enumerate() {
            let want = format!("tile {i}\n");
            let mut got = vec![0u8; want.len()];
            s.read_exact(&mut got).unwrap();
            assert_eq!(got, want.as_bytes());
        }
        let (t1, m1) = threads_and_rss();
        eprintln!("#908: {n} tunnels open: threads {t0} -> {t1}, resident {} -> {} MB", m0 >> 20, m1 >> 20);
        assert!(t1 - t0 <= 40, "{n} tunnels took {} threads", t1 - t0);
        for s in &open {
            s.shutdown(Shutdown::Write).unwrap();
        }
        for mut s in open {
            let mut rest = Vec::new();
            s.read_to_end(&mut rest).unwrap();
            assert!(rest.is_empty());
        }
        let w = per_window(&[a.budget.ledger(), b.budget.ledger()]);
        assert_eq!(w.values().sum::<u32>(), (0..n).filter(|i| i % 6 < 2).count() as u32);
        for ((class, win), k) in &w {
            assert!(*k <= class.rate().tokens, "{class:?} window {win}: {k}");
        }
    }

    /// Task #908: a client that drops its tunnel (no end told, just gone) frees its connection out
    /// for the next in line, and its host's count of those open; once both have ended, no slot is
    /// held.
    #[test]
    fn a_dropped_connection_frees_its_slot() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(StepClock::at(T0));
        let up = upstream_one_thread();
        let long = Duration::from_secs(60);
        let b = Arc::new(
            Budget::new(&store::nasfs::NasRoot::local_for_test(dir.path()), "m-a", clock.clone()).with_ledger(),
        );
        let limits = Limits { head: long, idle: long, conns: 1, wait: long };
        let p = Proxy::start_with(b, Some(Arc::new(move |_: &str, _: u16| Some(up))), limits).unwrap();
        let mut first = TcpStream::connect(p.addr).unwrap();
        write!(first, "CONNECT s3.amazonaws.com:443 HTTP/1.1\r\n\r\n").unwrap();
        let mut head = [0u8; 39];
        first.read_exact(&mut head).unwrap();
        let addr = p.addr;
        let next = std::thread::spawn(move || tunnel(&addr, "s3.amazonaws.com", b"next"));
        while p.counts.waited.load(Ordering::Relaxed) == 0 {
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(first);
        assert_eq!(next.join().unwrap(), b"next");
        assert_eq!(p.counts.refused.load(Ordering::Relaxed), 0);
        // (Its slot let go of as its task ends, just after its client saw the end.)
        let t = Instant::now();
        while slots(&p.ctx).live > 0 || slots(&p.ctx).clients > 0 {
            assert!(t.elapsed() < Duration::from_secs(10), "a slot still held");
            std::thread::sleep(Duration::from_millis(5));
        }
        let v = p.net.view();
        assert_eq!(v["hosts"]["s3.amazonaws.com"]["open"].as_u64(), Some(0), "{v}");
        assert_eq!(v["hosts"]["s3.amazonaws.com"]["conns"].as_u64(), Some(2), "{v}");
    }
}
