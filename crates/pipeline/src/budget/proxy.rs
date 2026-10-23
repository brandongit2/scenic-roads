//! The agent's proxy (docs/pool.md §7.7): every job's connections out go through it (`HTTPS_PROXY`
//! and the others set for each job, the job's sandbox denying any other), and those to a metered
//! host (crate::budget::class_of) take a token of the pool's budget before they're opened. A
//! tunnel (`CONNECT`, every HTTPS request's) is metered as one request: what's inside is the
//! client's TLS, end to end, never read (its certificate checks are the client's, unchanged). A
//! plain HTTP request (`GET http://…`) is passed on as one request, its connection closed after
//! it. It listens on this Mac alone (127.0.0.1). A process that sees a 429 or a `Retry-After`
//! tells it (`POST /hold?s=<seconds>`: crate::budget::seen), and the budget holds every process and Mac.

use super::{Budget, Class};
use anyhow::{Context, Result};
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

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

/// What the proxy has done, for the status and the tests.
#[derive(Default)]
pub struct Counts {
    /// Connections opened to a metered host, each class's.
    pub wiki: AtomicU64,
    pub query: AtomicU64,
    /// Connections opened to any other host.
    pub other: AtomicU64,
    /// Holds told.
    pub holds: AtomicU64,
    /// Connections that waited their turn (`Limits::conns` under way).
    pub waited: AtomicU64,
    /// Connections refused: too many clients, or a turn waited `Limits::wait` in vain.
    pub refused: AtomicU64,
}

/// What every connection's thread shares.
struct Ctx {
    budget: Arc<Budget>,
    counts: Arc<Counts>,
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
        let ctx = Arc::new(Ctx {
            budget: budget.clone(),
            counts: counts.clone(),
            resolve,
            limits,
            slots: Default::default(),
            holding: AtomicBool::new(false),
            hold_thread: Default::default(),
        });
        let (s, c) = (stop.clone(), ctx.clone());
        let thread = std::thread::Builder::new().name("budgets-proxy".into()).spawn(move || serve(listener, c, s))?;
        Ok(Proxy { addr, budget, counts, stop, thread: Some(thread), ctx })
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

    /// What it has done, for the status.
    pub fn view(&self) -> super::View {
        let held = self.budget.held_until();
        super::View {
            proxy: self.url(),
            wiki: self.counts.wiki.load(Ordering::Relaxed),
            query: self.counts.query.load(Ordering::Relaxed),
            other: self.counts.other.load(Ordering::Relaxed),
            holds: self.counts.holds.load(Ordering::Relaxed),
            held_until_ms: (held > self.budget.now_ms()).then_some(held),
            away: self.budget.away(),
        }
    }

    /// The port it listens on.
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Its address as a job is told it (`http://127.0.0.1:<port>`).
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The environment a job is given: its connections through the proxy (each name a client may
    /// read), but this Mac's own (the coordinator's, on 127.0.0.1), and where to tell a 429.
    pub fn env(&self) -> Vec<(String, String)> {
        let u = self.url();
        let mut v = Vec::new();
        for k in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"] {
            v.push((k.to_string(), u.clone()));
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
    told: std::sync::Condvar,
}

fn slots(ctx: &Ctx) -> std::sync::MutexGuard<'_, Slots> {
    ctx.slots.lock().unwrap_or_else(|e| e.into_inner())
}

/// A client's thread: let go of as it ends, however it ends.
struct Client(Arc<Ctx>);

impl Drop for Client {
    fn drop(&mut self) {
        slots(&self.0).clients -= 1;
    }
}

/// One of the `Limits::conns` connections out: as it ends, however it ends (a panic too), given to
/// the turn first in line, else freed.
struct Slot<'a>(&'a Ctx);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        let mut sl = slots(self.0);
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
fn slot(ctx: &Ctx) -> Option<Slot<'_>> {
    let mut sl = slots(ctx);
    if sl.live < ctx.limits.conns && sl.turns.is_empty() {
        sl.live += 1;
        return Some(Slot(ctx));
    }
    ctx.counts.waited.fetch_add(1, Ordering::Relaxed);
    let t = Arc::new(Turn::default());
    sl.turns.push_back(t.clone());
    let (mut sl, _) = t
        .told
        .wait_timeout_while(sl, ctx.limits.wait, |_| !t.given.load(Ordering::SeqCst))
        .unwrap_or_else(|e| e.into_inner());
    if t.given.load(Ordering::SeqCst) {
        return Some(Slot(ctx));
    }
    sl.turns.retain(|x| !Arc::ptr_eq(x, &t));
    None
}

/// Answered 503 and closed (never waiting on the client).
fn refuse(s: &mut TcpStream, ctx: &Ctx) -> io::Result<()> {
    ctx.counts.refused.fetch_add(1, Ordering::Relaxed);
    s.set_nonblocking(true)?;
    answer(s, "503 Service Unavailable", "too many connections through the proxy at once\n")?;
    s.shutdown(Shutdown::Write)
}

/// A client taken: served on a thread of its own while fewer than twice `conns` are.
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
    // (Not spawned: the client dropped, and with it its count.)
    let _ = std::thread::Builder::new().name("budgets-conn".into()).spawn(move || {
        // (A client gone, a host that didn't answer: the client sees it.)
        let _ = handle(s, &client.0);
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

fn read_head(s: &mut TcpStream) -> io::Result<Head> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > HEAD_MAX {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "head too long"));
        }
        let n = s.read(&mut chunk)?;
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

fn answer(s: &mut TcpStream, status: &str, body: &str) -> io::Result<()> {
    write!(
        s,
        "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
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

fn open(host: &str, port: u16, resolve: Option<&Resolve>) -> io::Result<TcpStream> {
    if let Some(a) = resolve.and_then(|r| r(host, port)) {
        return TcpStream::connect_timeout(&a, CONNECT_WAIT);
    }
    let mut last = io::Error::new(io::ErrorKind::NotFound, format!("{host}: no address"));
    for a in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&a, CONNECT_WAIT) {
            Ok(s) => return Ok(s),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// A token for `host`'s connection, when it's metered; its class.
fn meter(host: &str, budget: &Budget, counts: &Counts) -> Option<Class> {
    let class = super::class_of(host);
    match class {
        Some(c) => {
            budget.take(c);
            match c {
                Class::Wiki => counts.wiki.fetch_add(1, Ordering::Relaxed),
                Class::Query => counts.query.fetch_add(1, Ordering::Relaxed),
            };
        }
        None => {
            counts.other.fetch_add(1, Ordering::Relaxed);
        }
    }
    class
}

fn handle(mut s: TcpStream, ctx: &Arc<Ctx>) -> io::Result<()> {
    let (budget, counts, resolve) = (&*ctx.budget, &*ctx.counts, ctx.resolve.as_ref());
    s.set_nonblocking(false)?;
    s.set_read_timeout(Some(ctx.limits.head))?;
    let head = read_head(&mut s)?;
    // (From here on a connection idle that long ends.)
    s.set_read_timeout(Some(ctx.limits.idle))?;
    if head.method.eq_ignore_ascii_case("CONNECT") {
        let Some((host, port)) = host_port(&head.target, 443) else {
            return answer(&mut s, "400 Bad Request", "CONNECT host:port\n");
        };
        let Some(_slot) = slot(ctx) else { return refuse(&mut s, ctx) };
        let class = meter(&host, budget, counts);
        let began = budget.now_ms();
        let up = match open(&host, port, resolve).and_then(|u| u.set_read_timeout(Some(ctx.limits.idle)).map(|()| u)) {
            Ok(u) => u,
            Err(e) => return answer(&mut s, "502 Bad Gateway", &format!("{host}:{port}: {e}\n")),
        };
        // (HTTP/1.0: macOS's nc, the ssh tunnel's ProxyCommand (osmpass::streamed), takes no other.)
        s.write_all(b"HTTP/1.0 200 Connection established\r\n\r\n")?;
        let mut up_w = up.try_clone()?;
        if !head.rest.is_empty() {
            up_w.write_all(&head.rest)?;
        }
        return relay(s, up, || charge(budget, class, began));
    }
    // (The proxy's own: a 429 told.)
    if head.target.starts_with('/') {
        if let Some(q) = head.target.strip_prefix("/hold") {
            let secs = q
                .trim_start_matches('?')
                .split('&')
                .find_map(|kv| kv.strip_prefix("s="))
                .and_then(|v| v.parse::<f64>().ok())
                .map_or(super::HOLD_DEFAULT_S, |v| v.ceil().max(1.0) as u64);
            counts.holds.fetch_add(1, Ordering::Relaxed);
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
            return answer(&mut s, "200 OK", "held\n");
        }
        return answer(&mut s, "404 Not Found", "a proxy: CONNECT, an absolute URL, or /hold\n");
    }
    // A plain HTTP request by its absolute URL: passed on, its connection closed after it.
    let Some(after) = head.target.strip_prefix("http://") else {
        return answer(&mut s, "400 Bad Request", "only http:// URLs, or CONNECT\n");
    };
    let (hp, path) = match after.find('/') {
        Some(i) => (&after[..i], &after[i..]),
        None => (after, "/"),
    };
    let Some((host, port)) = host_port(hp, 80) else { return answer(&mut s, "400 Bad Request", "bad host\n") };
    let Some(_slot) = slot(ctx) else { return refuse(&mut s, ctx) };
    let class = meter(&host, budget, counts);
    let began = budget.now_ms();
    let mut up = match open(&host, port, resolve).and_then(|u| u.set_read_timeout(Some(ctx.limits.idle)).map(|()| u)) {
        Ok(u) => u,
        Err(e) => return answer(&mut s, "502 Bad Gateway", &format!("{host}:{port}: {e}\n")),
    };
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
    up.write_all(out.as_bytes())?;
    up.write_all(&head.rest)?;
    relay(s, up, || charge(budget, class, began))
}

/// A metered connection begun at `began` (the budget's clock) has ended: its time charged.
fn charge(budget: &Budget, class: Option<Class>, began: u64) {
    if let Some(c) = class {
        budget.charge(c, budget.now_ms().saturating_sub(began));
    }
}

/// Copies each way until both have ended (a way idle its read timeout ends: the client's told to
/// the host as its end; the host's shuts the whole connection), `done` run before the client's side is closed (what it
/// does is done before the client sees its answer end).
fn relay(client: TcpStream, up: TcpStream, done: impl FnOnce()) -> io::Result<()> {
    let (mut c_r, mut u_w) = (client.try_clone()?, up.try_clone()?);
    let t = std::thread::Builder::new().name("budgets-relay".into()).spawn(move || {
        // (The client's side ended, idle or broken: told to the host as its end, whose answer still
        // comes back; a host that then stays idle too is ended by its own read's timeout.)
        let _ = io::copy(&mut c_r, &mut u_w);
        let _ = u_w.shutdown(Shutdown::Write);
    })?;
    let (mut u_r, mut c_w) = (up, client);
    if io::copy(&mut u_r, &mut c_w).is_err() {
        let _ = u_r.shutdown(Shutdown::Both);
        let _ = c_w.shutdown(Shutdown::Read);
    }
    done();
    let _ = c_w.shutdown(Shutdown::Write);
    t.join().ok();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{Clock, Source, StepClock, Token};
    use super::*;
    use std::collections::BTreeMap;
    use std::path::Path;

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

    fn proxy(root: &Path, me: &str, clock: &Arc<StepClock>, up: &Upstream) -> Proxy {
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
        let (a, b) = (proxy(dir.path(), "m-a", &clock, &up), proxy(dir.path(), "m-b", &clock, &up));
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
        let n = |p: &Proxy| p.counts.wiki.load(Ordering::Relaxed) + p.counts.query.load(Ordering::Relaxed);
        assert_eq!(n(&a) + n(&b), 120);
        assert_eq!(a.counts.other.load(Ordering::Relaxed) + b.counts.other.load(Ordering::Relaxed), 0);
    }

    /// A 429 told to one agent's proxy holds the other's connections till its wait ends.
    #[test]
    fn a_429_told_to_one_proxy_holds_the_other() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(StepClock::at(T0));
        let up = upstream(Arc::new(|| {}));
        let (a, b) = (proxy(dir.path(), "m-a", &clock, &up), proxy(dir.path(), "m-b", &clock, &up));
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
        let first = Proxy::start(Arc::new(Budget::new(dir.path(), "m-a", Arc::new(StepClock::at(T0)))), None).unwrap();
        let port = first.port();
        assert!(listen(port).is_err());
        first.stop();
        assert!(listen(port).is_ok());
        let plain = store::spawn::listen("127.0.0.1:0").unwrap();
        assert!(listen(plain.local_addr().unwrap().port()).is_err());
    }

    fn proxy_within(root: &Path, clock: &Arc<StepClock>, up: &Upstream, limits: Limits) -> Proxy {
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
        let h =
            proxy_within(&dir.path().join("h"), &clock, &up, Limits { head: short, idle: long, conns: 8, wait: long });
        let mut out = Vec::new();
        TcpStream::connect(h.addr).unwrap().read_to_end(&mut out).unwrap();
        assert!(out.is_empty(), "{}", String::from_utf8_lossy(&out));
        // (A query's tunnel, its byte sent with its head; the client never ends its side.)
        let a = proxy_within(dir.path(), &clock, &up, Limits { head: long, idle: short, conns: 8, wait: long });
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
            &dir.path().join("b"),
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
        let a = proxy_within(dir.path(), &clock, &up, Limits { head: long, idle: long, conns: 4, wait: long });
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
        assert_eq!(a.counts.wiki.load(Ordering::Relaxed), 4);
        assert_eq!(a.counts.other.load(Ordering::Relaxed), 4);
        assert_eq!(a.budget.ledger().len(), 4);
    }

    /// A hold told to a proxy whose NAS is away is held there at once all the same (told is told).
    #[test]
    fn a_hold_is_held_before_the_nas_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(StepClock::at(T0));
        let up = upstream(Arc::new(|| {}));
        let a = proxy(&dir.path().join("away"), "m-a", &clock, &up);
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
        let a = proxy(dir.path(), "m-a", &clock, &up);
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
        let a = proxy(dir.path(), "m-a", &clock, &up);
        let r = request(&a.addr, "GET http://example.test:8080/a/b?c=1 HTTP/1.1\r\nHost: example.test:8080\r\nProxy-Connection: keep-alive\r\n\r\n");
        assert!(r.starts_with("GET /a/b?c=1 HTTP/1.1\r\n"), "{r}");
        assert!(r.contains("Connection: close\r\n") && !r.contains("Proxy-Connection"), "{r}");
        assert_eq!(a.counts.other.load(Ordering::Relaxed), 1);
        assert!(a.budget.ledger().is_empty());
        let r = request(&a.addr, "GET http://dumps.wikimedia.org/x HTTP/1.1\r\nHost: dumps.wikimedia.org\r\n\r\n");
        assert!(r.starts_with("GET /x HTTP/1.1"), "{r}");
        assert_eq!(a.budget.ledger().len(), 1);
        assert_eq!(up.heads.lock().unwrap().len(), 2);
        assert_eq!(tunnel(&a.addr, "s3.amazonaws.com", b"z"), b"z");
        assert_eq!(a.budget.ledger().len(), 1);
        assert_eq!(a.counts.other.load(Ordering::Relaxed), 2);
    }
}
