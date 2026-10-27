//! Outside data by URL: the DEM servers' files, ESA WorldCover's, GSI's tiles. A `Fetch` opens a
//! URL's file for range reads, or gets a small one whole, and tells a file the server doesn't have
//! (an answer, kept) from one it can't reach now (an error: the step fails and runs again later).
//!
//! `Fetcher` reads, in turn: a mirror folder (`<dir>/<host>/<path>`: a file, or the byte ranges of
//! one, a caller supplied, listed in `<path>.ranges`; `<path>.none` for one the server doesn't
//! have), then the network: natively HTTP, with the build's User-Agent, retries and a block cache;
//! WebAssembly has none (docs/workers.md), so there a task's caller supplies what it reads. It can
//! record what it fetched into a mirror folder, for a later run, or a worker, to read instead.
//! What it fetches is published data (`Kind::Data`): no robots.txt is read.
//!
//!   SCENIC_FETCH_MIRROR=<dir>   read from this mirror first
//!   SCENIC_FETCH_RECORD=<dir>   record what's fetched over the network into this mirror
//!   SCENIC_FETCH_OFFLINE=1      no network (natively too)

use anyhow::{bail, ensure, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store::range::RangeRead;
use store::sys::PosIo;
use store::IoError;

/// Whether the internet may be asked, before any request to it: refused (an error naming `url`)
/// under SCENIC_FETCH_OFFLINE=1, or once a test said so (`go_offline`), so a check on scratch data
/// never downloads, whatever path reaches the network. Every request to the internet asks this
/// first (tests/cache_accessor.rs checks each one does).
pub fn online(url: &str) -> Result<()> {
    // (This Mac itself isn't the internet: a coordinator's or a test server's.)
    let host = host_of(url);
    if matches!(host, "127.0.0.1" | "localhost" | "::1") {
        return Ok(());
    }
    if OFFLINE.load(std::sync::atomic::Ordering::Relaxed)
        || std::env::var("SCENIC_FETCH_OFFLINE").is_ok_and(|v| !v.is_empty() && v != "0")
    {
        bail!("{url}: no network here (SCENIC_FETCH_OFFLINE)");
    }
    Ok(())
}

/// `url`'s host (without its port, user or IPv6 brackets).
fn host_of(url: &str) -> &str {
    let host = url.split("://").nth(1).unwrap_or(url).split(['/', '?']).next().unwrap_or("");
    let host = host.rsplit_once('@').map_or(host, |h| h.1);
    if host.starts_with('[') {
        host.split(']').next().map(|h| &h[1..]).unwrap_or("")
    } else {
        host.split(':').next().unwrap_or("")
    }
}

/// What every error of a server that gave no answer says after its URL, however many retries it
/// took: the network or the server, not the build (`is_unreachable`).
const NO_ANSWER: &str = ": no answer (";

/// Whether `e` is a server that gave no answer (after its retries), anywhere in its chain or its
/// text (a range read's error reaches its reader as text): worth trying again later, not a bug.
pub fn is_unreachable(e: &anyhow::Error) -> bool {
    format!("{e:#}").contains(NO_ANSWER)
}

/// A program's exit status when a server it needs gave no answer (EX_UNAVAILABLE): the unit job
/// sets that z6 tile aside and goes on with its others (crate::unit::SourceDown).
pub const UNREACHABLE_EXIT: u8 = 69;

/// A program's end: its error printed as `main`'s would be, its status `UNREACHABLE_EXIT` when a
/// server gave no answer, else 1.
pub fn exit_with(r: Result<()>) -> std::process::ExitCode {
    match r {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e:?}");
            std::process::ExitCode::from(if is_unreachable(&e) { UNREACHABLE_EXIT } else { 1 })
        }
    }
}

/// The folder of hosts that gave no answer lately (SCENIC_FETCH_DOWN; the unit job sets it for its
/// programs): a file per host, written as a request gives up on it, so the requests after it (the
/// job's next z6 tiles' programs) don't each wait out their retries, minutes, on a server that's
/// down. The host is asked again once its note is `DOWN_S` old.
pub const DOWN_ENV: &str = "SCENIC_FETCH_DOWN";
pub const DOWN_S: u64 = 30 * 60;

// (WebAssembly has no network: crate::fetch::net is native only, and so these.)
#[cfg_attr(target_os = "wasi", allow(dead_code))]
fn down_dir() -> Option<PathBuf> {
    std::env::var_os(DOWN_ENV).filter(|d| !d.is_empty()).map(PathBuf::from)
}

#[cfg_attr(target_os = "wasi", allow(dead_code))]
fn down_note(dir: Option<&Path>, url: &str) -> Option<PathBuf> {
    // (Its host and port: two servers of one host are told apart. `host_port`, the ':' a '_'.)
    let auth = url.split("://").nth(1).unwrap_or(url).split(['/', '?', '#']).next().unwrap_or("");
    let auth = auth.rsplit_once('@').map_or(auth, |h| h.1);
    let key = auth.replace(['[', ']'], "").replace(':', "_");
    let plain = !host_of(url).is_empty() && !key.contains(['/', '\\']) && !key.starts_with('.');
    plain.then(|| dir.map(|d| d.join(key))).flatten()
}

/// An error at once when `url`'s host gave no answer less than `DOWN_S` ago (its note in `dir`).
#[cfg_attr(target_os = "wasi", allow(dead_code))]
fn down_check(dir: Option<&Path>, url: &str) -> Result<()> {
    let Some(p) = down_note(dir, url) else { return Ok(()) };
    let Ok(age) = std::fs::metadata(&p).and_then(|m| m.modified()).map(|t| t.elapsed().unwrap_or_default()) else {
        return Ok(());
    };
    if age.as_secs() < DOWN_S {
        let why = std::fs::read_to_string(&p).unwrap_or_default();
        bail!(
            "{url}{NO_ANSWER}its host gave none {} min ago: {why}; asked again {} min after that)",
            age.as_secs() / 60,
            DOWN_S / 60
        );
    }
    Ok(())
}

std::thread_local! {
    /// This thread's reader's stop flag (`cancellable`).
    static CANCEL: std::cell::RefCell<Option<Arc<std::sync::atomic::AtomicBool>>> = const { std::cell::RefCell::new(None) };
}

/// Runs `f` with `flag` as this thread's stop flag: once it's set, a request `f` makes gives up
/// (an error) before its next attempt and within a retry's wait, not after its retries (a reader
/// reading ahead, stopped, ends at once even while its host gives no answer).
pub fn cancellable<R>(flag: &Arc<std::sync::atomic::AtomicBool>, f: impl FnOnce() -> R) -> R {
    let before = CANCEL.with(|c| c.replace(Some(flag.clone())));
    struct Back(Option<Arc<std::sync::atomic::AtomicBool>>);
    impl Drop for Back {
        fn drop(&mut self) {
            let b = self.0.take();
            CANCEL.with(|c| *c.borrow_mut() = b);
        }
    }
    let _back = Back(before);
    f()
}

/// An error when this thread's reader stopped (`cancellable`).
#[cfg_attr(target_os = "wasi", allow(dead_code))]
fn cancelled(url: &str) -> Result<()> {
    if CANCEL.with(|c| c.borrow().as_ref().is_some_and(|f| f.load(std::sync::atomic::Ordering::Relaxed))) {
        bail!("{url}: given up (its reader stopped)");
    }
    Ok(())
}

/// A retry's wait of `secs` (each `second` long: a real second but in tests), in slices, ending
/// early in an error when this thread's reader stops; then the host's note checked again
/// (`down_check`: another request may have found it down).
#[cfg_attr(target_os = "wasi", allow(dead_code))]
fn retry_wait(dir: Option<&Path>, url: &str, secs: u64, second: std::time::Duration) -> Result<()> {
    // (Counted by the job's proxy, for its host: crate::budget::net.)
    crate::budget::retried(url);
    let end = std::time::Instant::now() + second * secs as u32;
    loop {
        cancelled(url)?;
        let now = std::time::Instant::now();
        if now >= end {
            break;
        }
        std::thread::sleep((end - now).min(std::time::Duration::from_millis(100)));
    }
    down_check(dir, url)
}

/// Notes in `dir` that `url`'s host gave no answer (`why`), for `down_check`.
#[cfg_attr(target_os = "wasi", allow(dead_code))]
fn note_down(dir: Option<&Path>, url: &str, why: &str) {
    if let Some(p) = down_note(dir, url) {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).ok();
        }
        std::fs::write(&p, why).ok();
    }
}

#[cfg(all(test, not(target_os = "wasi")))]
#[test]
fn a_host_that_gave_no_answer_is_not_asked_again_for_a_while() {
    let d = tempfile::tempdir().unwrap();
    let url = "https://data.example.org/datasets/a.zip";
    let net = net::Net::new().with_down(Some(d.path().to_path_buf()));
    // Not noted: asked (and here, offline, refused as such: not a server's lack of answer).
    let e = net.open(url, None, None).err().unwrap();
    assert!(!is_unreachable(&e), "{e:#}");
    // Noted: an error at once, a server's lack of answer, naming why.
    note_down(Some(d.path()), url, "connection reset by peer");
    assert!(d.path().join("data.example.org").is_file());
    let e = net.open("https://data.example.org/other.zip", None, None).err().unwrap();
    assert!(is_unreachable(&e) && format!("{e:#}").contains("connection reset by peer"), "{e:#}");
    // Another host: asked.
    assert!(down_check(Some(d.path()), "https://other.example.org/a.zip").is_ok());
    // The same host on another port: another server, asked.
    assert!(down_check(Some(d.path()), "https://data.example.org:8443/a.zip").is_ok());
    note_down(Some(d.path()), "http://[::1]:9/x", "refused");
    assert!(d.path().join("__1_9").is_file());
    // An old note: asked again.
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(DOWN_S + 60);
    std::fs::File::options().write(true).open(d.path().join("data.example.org")).unwrap().set_modified(old).unwrap();
    assert!(down_check(Some(d.path()), url).is_ok());
    // No folder: nothing noted, nothing refused; nor a host that would name another file.
    note_down(None, url, "x");
    assert!(down_check(None, url).is_ok());
    assert!(down_note(Some(d.path()), "https://../x").is_none());
}

#[cfg(test)]
#[test]
fn offline_refuses_the_internet_not_this_mac() {
    go_offline();
    assert!(online("https://s3.amazonaws.com/elevation-tiles-prod/terrarium/1/0/0.png").is_err());
    assert!(online("http://example.org:8080/x").is_err());
    for u in ["http://127.0.0.1:18270/net/x", "http://localhost/x", "http://[::1]:9/x"] {
        assert!(online(u).is_ok(), "{u}");
    }
}

/// (The unit tests' process is offline from its start, whichever test runs first: none reaches the
/// internet.)
static OFFLINE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(cfg!(test));

/// No network for the rest of this process (`online`): a test's.
pub fn go_offline() {
    OFFLINE.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Every request names the map, as the steps' own do.
pub const USER_AGENT: &str = "scenic-roads/0.1 (personal offline map)";

/// What a fetched source is (docs/inputs.md §6.9), which decides whether its host's robots.txt is
/// read first: robots.txt is for crawlers, so only a page's is. An API or a data download follows
/// its provider's usage policy instead: `USER_AGENT`, the pool's rate limits (crate::budget), a
/// back-off on 429, the credit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A documented API: a query service, a WFS or WCS, MediaWiki's API, an ArcGIS REST service.
    Api,
    /// A published data download: a dump, an extract, a dataset's files.
    Data,
    /// A web page, an undocumented URL, links followed or scraped: its robots.txt read first.
    Page,
}

/// Outside data by URL.
pub trait Fetch: Send + Sync {
    /// `url`'s file for range reads; None when the server says it has none (404, 403 or 410: S3's
    /// answers for a key that doesn't exist).
    fn open(&self, url: &str) -> Result<Option<Arc<dyn RangeRead>>>;

    /// `url`'s file whole (a small one); None when the server answers 404.
    fn get(&self, url: &str) -> Result<Option<Vec<u8>>>;

    /// As `get`, each request over the network held to `within` (and tried again, as a failed
    /// one is): a connection that goes silent before its answer's head fails fast, while a body
    /// coming slowly but steadily is waited for. A reader of many small files (a pyramid's tiles)
    /// asks this; a fetch without a network (a mirror, the tests') is `get`.
    fn get_within(&self, url: &str, within: Within) -> Result<Option<Vec<u8>>> {
        let _ = within;
        self.get(url)
    }
}

/// A small file's request's limits (`Fetch::get_within`): its answer's head within `head` of
/// the request being sent, then its whole body within `most / floor` seconds (`body`: the time
/// a body of `most` bytes takes at `floor` bytes a second), the head's wait not counted. It's a
/// time for the body in all, not a rate checked as it comes: a body may stall and resume, so
/// long as it's all in by then.
#[derive(Clone, Copy, Debug)]
pub struct Within {
    pub head: std::time::Duration,
    pub floor: u64,
    pub most: u64,
}

impl Within {
    /// The time a body is given.
    pub fn body(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(self.most as f64 / self.floor.max(1) as f64)
    }
}

/// Byte ranges (start, end), sorted and merged.
#[derive(Clone, Debug, Default)]
struct Ranges(Vec<(u64, u64)>);

impl Ranges {
    #[cfg(test)]
    fn add(&mut self, s: u64, e: u64) {
        if s >= e {
            return;
        }
        self.0.push((s, e));
        self.merge();
    }

    /// Sorts and merges the ranges.
    fn merge(&mut self) {
        self.0.retain(|r| r.0 < r.1);
        self.0.sort_unstable();
        let mut out: Vec<(u64, u64)> = Vec::with_capacity(self.0.len());
        for &(s, e) in &self.0 {
            match out.last_mut() {
                Some(l) if s <= l.1 => l.1 = l.1.max(e),
                _ => out.push((s, e)),
            }
        }
        self.0 = out;
    }

    fn covers(&self, s: u64, e: u64) -> bool {
        let i = self.0.partition_point(|r| r.1 < e);
        self.0.get(i).is_some_and(|r| r.0 <= s && r.1 >= e)
    }

    /// From a `.ranges` file's lines ("start end").
    fn parse(text: &str) -> Ranges {
        let mut r = Ranges::default();
        for l in text.lines() {
            let mut it = l.split_ascii_whitespace().map(str::parse::<u64>);
            if let (Some(Ok(s)), Some(Ok(e))) = (it.next(), it.next()) {
                r.0.push((s, e));
            }
        }
        r.merge();
        r
    }
}

/// Where `url`'s file lies in mirror folder `root`: `<root>/<host>/<path>`.
pub fn mirror_path(root: &Path, url: &str) -> Result<PathBuf> {
    let rest = url.split_once("://").map_or(url, |x| x.1);
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let mut p = root.to_path_buf();
    for part in rest.split('/').filter(|s| !s.is_empty() && *s != ".") {
        ensure!(part != "..", "{url}: not a plain URL");
        p.push(part);
    }
    ensure!(p != root, "{url}: no path");
    Ok(p)
}

/// `p` with `ext` appended to its name.
fn beside(p: &Path, ext: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(ext);
    PathBuf::from(s)
}

/// A mirror folder's file for one URL: which of its bytes are there.
struct Local {
    file: std::fs::File,
    len: u64,
    /// None: all of them.
    have: Option<Ranges>,
}

enum Found {
    Absent,
    File(Local),
}

fn lookup(root: &Path, url: &str) -> Result<Option<Found>> {
    let p = mirror_path(root, url)?;
    if beside(&p, ".none").exists() {
        return Ok(Some(Found::Absent));
    }
    let Ok(file) = std::fs::File::open(&p) else { return Ok(None) };
    let len = file.metadata()?.len();
    let have = std::fs::read_to_string(beside(&p, ".ranges")).ok().map(|t| Ranges::parse(&t));
    Ok(Some(Found::File(Local { file, len, have })))
}

/// A URL's file: read from the mirror where it has the bytes, else over the network by blocks.
struct Remote {
    url: String,
    len: u64,
    local: Option<Local>,
    #[cfg(not(target_os = "wasi"))]
    online: Option<net::Online>,
}

impl RangeRead for Remote {
    fn len(&self) -> Result<u64, IoError> {
        Ok(self.len)
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
        if off.checked_add(len as u64).is_none_or(|e| e > self.len) {
            return Err(IoError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("{}: bytes {off}..+{len} are past the end ({} bytes)", self.url, self.len),
            )));
        }
        if len == 0 {
            return Ok(Vec::new());
        }
        if let Some(l) = &self.local {
            if l.have.as_ref().is_none_or(|h| h.covers(off, off + len as u64)) {
                let mut b = vec![0u8; len];
                l.file.read_exact_at(&mut b, off)?;
                return Ok(b);
            }
        }
        #[cfg(not(target_os = "wasi"))]
        if let Some(o) = &self.online {
            return o.read(&self.url, self.len, off, len);
        }
        Err(IoError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{}: bytes {off}..+{len} weren't supplied (no network here)", self.url),
        )))
    }
}

/// Mirror folders, then the network (natively).
pub struct Fetcher {
    mirror: Option<PathBuf>,
    #[cfg(not(target_os = "wasi"))]
    net: Option<(net::Net, Option<PathBuf>)>,
}

impl Fetcher {
    /// A fetcher reading `mirror` first, then (natively, when `online`) the network, recording
    /// what that fetched into `record`. (WebAssembly has no network.)
    pub fn new(mirror: Option<PathBuf>, record: Option<PathBuf>, online: bool) -> Fetcher {
        #[cfg(target_os = "wasi")]
        let _ = (record, online);
        Fetcher {
            mirror,
            #[cfg(not(target_os = "wasi"))]
            net: online.then(|| (net::Net::new(), record)),
        }
    }

    /// A fetcher reading the network alone over HTTPS, following no redirect: the coordinator's,
    /// for tasks' reads of the data servers it allows (none of them redirects).
    #[cfg(not(target_os = "wasi"))]
    pub fn strict() -> Fetcher {
        Fetcher { mirror: None, net: Some((net::Net::strict(), None)) }
    }

    /// From SCENIC_FETCH_MIRROR, SCENIC_FETCH_RECORD and SCENIC_FETCH_OFFLINE.
    pub fn from_env() -> Fetcher {
        let dir = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let offline = std::env::var("SCENIC_FETCH_OFFLINE").is_ok_and(|v| !v.is_empty() && v != "0");
        Fetcher::new(dir("SCENIC_FETCH_MIRROR"), dir("SCENIC_FETCH_RECORD"), !offline)
    }
}

impl Fetch for Fetcher {
    fn open(&self, url: &str) -> Result<Option<Arc<dyn RangeRead>>> {
        let mut local = None;
        if let Some(m) = &self.mirror {
            match lookup(m, url)? {
                Some(Found::Absent) => return Ok(None),
                Some(Found::File(l)) => local = Some(l),
                None => {}
            }
        }
        #[cfg(not(target_os = "wasi"))]
        if let Some((n, record)) = &self.net {
            if local.as_ref().is_none_or(|l| l.have.is_some()) {
                return n.open(url, local, record.as_deref());
            }
        }
        match local {
            Some(l) => Ok(Some(Arc::new(Remote {
                url: url.to_string(),
                len: l.len,
                local: Some(l),
                #[cfg(not(target_os = "wasi"))]
                online: None,
            }))),
            None => bail!("{url}: not supplied, and there's no network here"),
        }
    }

    fn get(&self, url: &str) -> Result<Option<Vec<u8>>> {
        self.get_by(url, None)
    }

    fn get_within(&self, url: &str, within: Within) -> Result<Option<Vec<u8>>> {
        self.get_by(url, Some(within))
    }
}

impl Fetcher {
    /// `get`, a request over the network held to `deadline` (`Fetch::get_within`).
    fn get_by(&self, url: &str, deadline: Option<Within>) -> Result<Option<Vec<u8>>> {
        let _ = deadline;
        if let Some(m) = &self.mirror {
            match lookup(m, url)? {
                Some(Found::Absent) => return Ok(None),
                Some(Found::File(l)) if l.have.as_ref().is_none_or(|h| h.covers(0, l.len)) => {
                    let mut b = vec![0u8; l.len as usize];
                    l.file.read_exact_at(&mut b, 0)?;
                    return Ok(Some(b));
                }
                _ => {}
            }
        }
        #[cfg(not(target_os = "wasi"))]
        if let Some((n, record)) = &self.net {
            return n.get_recorded(url, record.as_deref(), deadline);
        }
        bail!("{url}: not supplied, and there's no network here")
    }
}

/// A file whose reads are noted, so they can be kept in a mirror folder (`save`): a run elsewhere
/// given that folder (SCENIC_FETCH_MIRROR) reads just those bytes, without the file.
pub struct Noted {
    inner: Arc<dyn RangeRead>,
    read: std::sync::Mutex<Vec<(u64, u64)>>,
}

impl Noted {
    pub fn new(inner: Arc<dyn RangeRead>) -> Arc<Noted> {
        Arc::new(Noted { inner, read: Default::default() })
    }

    /// Keeps the bytes read so far in mirror folder `root` as `url`'s (beside any it has): the file
    /// at its length, holding them, and their ranges listed (`<path>.ranges`). The bytes kept.
    pub fn save(&self, root: &Path, url: &str) -> Result<u64> {
        let p = mirror_path(root, url)?;
        if p.exists() && !beside(&p, ".ranges").exists() {
            return Ok(0); // a whole file already
        }
        let mut read = Ranges(self.read.lock().unwrap().clone());
        read.merge();
        let mut all = std::fs::read_to_string(beside(&p, ".ranges")).map(|t| Ranges::parse(&t)).unwrap_or_default();
        std::fs::create_dir_all(p.parent().context("a mirror path without a folder")?)?;
        let len = self.inner.len()?;
        let f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&p)
            .with_context(|| p.display().to_string())?;
        if f.metadata()?.len() != len {
            f.set_len(len)?;
        }
        let mut n = 0;
        for &(s, e) in &read.0 {
            f.write_all_at(&self.inner.read_at(s, (e - s) as usize)?, s)?;
            all.0.push((s, e));
            n += e - s;
        }
        all.merge();
        let text: String = all.0.iter().map(|(s, e)| format!("{s} {e}\n")).collect();
        std::fs::write(beside(&p, ".ranges"), text)?;
        Ok(n)
    }
}

impl RangeRead for Noted {
    fn len(&self) -> Result<u64, IoError> {
        self.inner.len()
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
        let b = self.inner.read_at(off, len)?;
        self.read.lock().unwrap().push((off, off + len as u64));
        Ok(b)
    }
}

/// Keeps in mirror folder `root` that `url`'s file isn't there (`<path>.none`).
pub fn save_none(root: &Path, url: &str) -> Result<()> {
    let p = mirror_path(root, url)?;
    std::fs::create_dir_all(p.parent().context("a mirror path without a folder")?)?;
    std::fs::write(beside(&p, ".none"), b"")?;
    Ok(())
}

/// Outside data held in memory (tests).
#[derive(Default)]
pub struct MapFetch(pub std::collections::BTreeMap<String, Option<Vec<u8>>>);

impl Fetch for MapFetch {
    fn open(&self, url: &str) -> Result<Option<Arc<dyn RangeRead>>> {
        match self.0.get(url) {
            Some(Some(b)) => Ok(Some(Arc::new(b.clone()))),
            Some(None) => Ok(None),
            None => bail!("{url}: not supplied"),
        }
    }

    fn get(&self, url: &str) -> Result<Option<Vec<u8>>> {
        match self.0.get(url) {
            Some(b) => Ok(b.clone()),
            None => bail!("{url}: not supplied"),
        }
    }
}

/// The bytes this process has fetched over the network so far (none in WebAssembly, where a page
/// reads through the coordinator): a phase counts the difference across it.
pub fn fetched_bytes() -> u64 {
    #[cfg(not(target_os = "wasi"))]
    {
        net::FETCHED.load(std::sync::atomic::Ordering::Relaxed)
    }
    #[cfg(target_os = "wasi")]
    {
        0
    }
}

#[cfg(not(target_os = "wasi"))]
mod net {
    //! HTTP: range requests with retries (as the Python steps' GDAL did: 10 retries, from 2 s,
    //! doubling), small files whole (as their GSI fetch did: 8 tries, 2 s more each time); and
    //! the recording of what's fetched into a mirror folder.

    use super::*;
    use std::collections::HashMap;
    use std::io::Write;
    use std::sync::Mutex;
    use std::time::Duration;

    /// A fetched file's blocks (requests are whole blocks), how much a small read fetches, and
    /// how much of a file is kept in memory.
    const BLOCK: u64 = 16 << 10;
    const READ_AHEAD: u64 = 64 << 10;
    const CACHE_BYTES: usize = 16 << 20;

    /// Writes what's fetched into a mirror folder: a file of the URL's length with the fetched
    /// ranges in place (sparse), the ranges listed beside it; or a whole file; or the server's
    /// "none".
    pub struct Recorder {
        path: PathBuf,
        len: u64,
    }

    impl Recorder {
        pub fn new(root: &Path, url: &str, len: u64) -> Result<Recorder> {
            Ok(Recorder { path: mirror_path(root, url)?, len })
        }

        pub fn range(&self, s: u64, b: &[u8]) -> Result<()> {
            if self.path.exists() && !beside(&self.path, ".ranges").exists() {
                return Ok(()); // a whole file already
            }
            std::fs::create_dir_all(self.path.parent().unwrap())?;
            let f = std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(&self.path)?;
            if f.metadata()?.len() != self.len {
                f.set_len(self.len)?;
            }
            f.write_all_at(b, s)?;
            let mut r = std::fs::OpenOptions::new().append(true).create(true).open(beside(&self.path, ".ranges"))?;
            r.write_all(format!("{s} {}\n", s + b.len() as u64).as_bytes())?;
            Ok(())
        }

        pub fn whole(root: &Path, url: &str, b: &[u8]) -> Result<()> {
            let p = mirror_path(root, url)?;
            std::fs::create_dir_all(p.parent().unwrap())?;
            let tmp = beside(&p, &format!(".{}.tmp", store::sys::pid()));
            std::fs::write(&tmp, b)?;
            std::fs::rename(&tmp, &p)?;
            std::fs::remove_file(beside(&p, ".ranges")).ok();
            Ok(())
        }

        pub fn none(root: &Path, url: &str) -> Result<()> {
            let p = mirror_path(root, url)?;
            std::fs::create_dir_all(p.parent().unwrap())?;
            std::fs::write(beside(&p, ".none"), b"")?;
            Ok(())
        }
    }

    /// Fetched blocks kept in memory, least recently used out first.
    #[derive(Default)]
    struct Blocks {
        map: HashMap<u64, (Arc<[u8]>, u64)>,
        tick: u64,
        bytes: usize,
    }

    impl Blocks {
        fn get(&mut self, i: u64) -> Option<Arc<[u8]>> {
            self.tick += 1;
            let t = self.tick;
            self.map.get_mut(&i).map(|e| {
                e.1 = t;
                e.0.clone()
            })
        }

        fn put(&mut self, i: u64, b: Arc<[u8]>) {
            self.tick += 1;
            self.bytes += b.len();
            if let Some(old) = self.map.insert(i, (b, self.tick)) {
                self.bytes -= old.0.len();
            }
            while self.bytes > CACHE_BYTES && self.map.len() > 1 {
                let oldest = *self.map.iter().min_by_key(|e| e.1 .1).unwrap().0;
                let (b, _) = self.map.remove(&oldest).unwrap();
                self.bytes -= b.len();
            }
        }
    }

    /// The bytes this process has fetched over the network (ranges and whole files), for a phase's
    /// count (`super::fetched_bytes`).
    pub(super) static FETCHED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn fetched(n: usize) {
        FETCHED.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
    }

    /// A file's network side: the connection pool, the blocks fetched, where they're recorded.
    pub struct Online {
        net: Net,
        record: Option<Recorder>,
        blocks: Mutex<Blocks>,
    }

    impl Online {
        /// `len` bytes at `off` of the `size`-byte file at `url`: from the blocks in memory, the
        /// missing ones fetched (a run of them in one request).
        pub fn read(&self, url: &str, size: u64, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
            let (first, last) = (off / BLOCK, (off + len as u64 - 1) / BLOCK);
            let nblocks = size.div_ceil(BLOCK);
            let mut have: HashMap<u64, Arc<[u8]>> = HashMap::new();
            let mut runs: Vec<(u64, u64)> = Vec::new();
            {
                let mut c = self.blocks.lock().unwrap();
                for i in first..=last {
                    match c.get(i) {
                        Some(b) => {
                            have.insert(i, b);
                        }
                        None => match runs.last_mut() {
                            Some(r) if r.1 + 1 == i => r.1 = i,
                            _ => runs.push((i, i)),
                        },
                    }
                }
                // A small read takes the next blocks along (a file's directories, a tile list).
                if len as u64 <= READ_AHEAD {
                    if let Some(r) = runs.last_mut() {
                        if r.1 == last {
                            let want = (r.0 + READ_AHEAD / BLOCK - 1).min(nblocks - 1);
                            while r.1 < want && !c.map.contains_key(&(r.1 + 1)) {
                                r.1 += 1;
                            }
                        }
                    }
                }
            }
            for &(a, b) in &runs {
                let (s, e) = (a * BLOCK, ((b + 1) * BLOCK).min(size));
                let bytes =
                    self.net.range(url, s, e).map_err(|e| IoError::Io(std::io::Error::other(format!("{e:#}"))))?;
                if let Some(r) = &self.record {
                    if let Err(e) = r.range(s, &bytes) {
                        eprintln!("fetch: {url}: not recorded: {e:#}");
                    }
                }
                let mut c = self.blocks.lock().unwrap();
                for (k, chunk) in bytes.chunks(BLOCK as usize).enumerate() {
                    let i = a + k as u64;
                    let block: Arc<[u8]> = Arc::from(chunk);
                    if (first..=last).contains(&i) {
                        have.insert(i, block.clone());
                    }
                    c.put(i, block);
                }
            }
            let mut out = Vec::with_capacity(len);
            for i in first..=last {
                let b = have
                    .get(&i)
                    .ok_or_else(|| IoError::Io(std::io::Error::other(format!("{url}: block {i} missing"))))?;
                let s = if i == first { (off - i * BLOCK) as usize } else { 0 };
                let e = if i == last { (off + len as u64 - i * BLOCK) as usize } else { b.len() };
                out.extend_from_slice(&b[s..e]);
            }
            Ok(out)
        }
    }

    /// Requests to one host this process has in flight at most, whichever readers ask (the
    /// elevation sampler's pool, its reading ahead, a pyramid's tile fetches): what one step asks
    /// of a server at once stays bounded however many threads read. A request holds its slot
    /// through its retries' waits. Modest, as the hosts read live are free services that publish no
    /// limit (GSI's cyberjapandata.gsi.go.jp among them, task #665 review): a pool of four jobs
    /// asks one host 64 at once at most.
    pub const HOST_REQUESTS: usize = 16;

    /// The requests in flight by host.
    static HOSTS: std::sync::LazyLock<(Mutex<HashMap<String, usize>>, std::sync::Condvar)> =
        std::sync::LazyLock::new(Default::default);

    /// One of a host's `HOST_REQUESTS` slots, given back when dropped.
    pub(super) struct HostSlot(String);

    impl HostSlot {
        /// A slot of `url`'s host, waiting while all of them are taken.
        pub(super) fn take(url: &str) -> HostSlot {
            let host = host_of(url).to_string();
            let (m, cv) = &*HOSTS;
            let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
            while g.get(&host).copied().unwrap_or(0) >= HOST_REQUESTS {
                g = cv.wait(g).unwrap_or_else(|e| e.into_inner());
            }
            *g.entry(host.clone()).or_default() += 1;
            HostSlot(host)
        }
    }

    impl Drop for HostSlot {
        fn drop(&mut self) {
            let (m, cv) = &*HOSTS;
            let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(n) = g.get_mut(&self.0) {
                *n -= 1;
                if *n == 0 {
                    g.remove(&self.0);
                }
            }
            cv.notify_all();
        }
    }

    #[derive(Clone)]
    pub struct Net {
        agent: ureq::Agent,
        /// The notes of hosts that gave no answer lately (SCENIC_FETCH_DOWN: super::down_check).
        down: Option<PathBuf>,
        /// A second of the retries' waits: a real one but in tests (`with_second`).
        second: Duration,
        /// This Mac's agent's proxy (`host:port`) that requests go through, when they do
        /// (the job's HTTPS_PROXY, a loopback address: `local_proxy`), for `proxy_back`.
        proxy: Option<String>,
        /// Whether the job is pausing (`control::draining_lately`, but in tests).
        draining: fn() -> bool,
    }

    /// How long a request waits for its Mac's agent's proxy to listen again (the agent restarting
    /// into a new app after a publish) before failing as any other: in the retries' seconds.
    pub(super) const PROXY_WAIT_S: u64 = 5 * 60;

    /// The proxy ureq takes from the environment (as `Self::config` does), when it's this Mac's
    /// own (a loopback address: the agent's), as `host:port`.
    fn local_proxy() -> Option<String> {
        let p = ureq::Proxy::try_from_env()?;
        let host = p.host().trim_start_matches('[').trim_end_matches(']');
        matches!(host, "127.0.0.1" | "localhost" | "::1").then(|| format!("{}:{}", p.host(), p.port()))
    }

    /// An answer that's worth asking again: too many requests, a server's error, or no answer.
    fn transient(code: u16) -> bool {
        code == 429 || code >= 500
    }

    fn header(r: &ureq::http::Response<ureq::Body>, name: &str) -> Option<String> {
        r.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string)
    }

    /// A Content-Range header's range (first and last byte) and the file's length.
    type ContentRange = (Option<(u64, u64)>, Option<u64>);

    /// "bytes s-e/total" (or "bytes */total").
    pub(super) fn content_range(v: &str) -> Option<ContentRange> {
        let v = v.trim().strip_prefix("bytes")?.trim();
        let (range, total) = v.split_once('/')?;
        let total = total.trim().parse().ok();
        let range = range.trim();
        if range == "*" {
            return Some((None, total));
        }
        let (s, e) = range.split_once('-')?;
        Some((Some((s.trim().parse().ok()?, e.trim().parse().ok()?)), total))
    }

    impl Net {
        pub fn new() -> Net {
            Net {
                agent: Self::config().build().into(),
                down: down_dir(),
                second: Duration::from_secs(1),
                proxy: local_proxy(),
                draining: crate::control::draining_lately,
            }
        }

        /// Through the proxy at `url` (`http://127.0.0.1:<port>`) in place of the environment's.
        #[cfg(test)]
        pub fn through(self, url: &str) -> Net {
            let agent = Self::config().proxy(Some(ureq::Proxy::new(url).unwrap())).build().into();
            Net { agent, proxy: crate::osmpass::proxy_addr(url), ..self }
        }

        /// With `draining` in place of `control::draining`.
        #[cfg(test)]
        pub fn with_draining(self, draining: fn() -> bool) -> Net {
            Net { draining, ..self }
        }

        /// After a failed attempt: when this Mac's agent's proxy refuses connections (the agent
        /// restarting), waits for it, polling its port each retry second; true once it listens
        /// again, so the attempt is made again without counting against the retries. `left` is
        /// the request's whole wait for its proxy (`PROXY_WAIT_S` at the start, `proxy_wait`),
        /// spent across every such wait: a proxy that keeps going away can't hold a request past
        /// it. False at once when the proxy listens (the remote host's failure, retried as ever),
        /// when it doesn't answer or fails otherwise (busy: only a refusal is an agent away),
        /// when there's no proxy, and when the wait is spent or the job is pausing
        /// (`control::draining_lately`): the retries go on as they would.
        /// An error when this thread's reader stops (`cancelled`).
        fn proxy_back(&self, url: &str, left: &mut Duration) -> Result<bool> {
            let Some(addr) = &self.proxy else { return Ok(false) };
            // (Refused at every address it names; any other outcome counts as listening.)
            let refused = || {
                use std::net::ToSocketAddrs;
                let addrs: Vec<_> = addr.to_socket_addrs().map(|a| a.collect()).unwrap_or_default();
                !addrs.is_empty()
                    && addrs.iter().all(|a| {
                        matches!(std::net::TcpStream::connect_timeout(a, Duration::from_secs(2)),
                            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused)
                    })
            };
            if left.is_zero() || !refused() {
                return Ok(false);
            }
            eprintln!("fetch: {url}: this Mac's agent ({addr}) isn't listening; waiting for it");
            let poll = self.second.min(Duration::from_millis(500));
            while !left.is_zero() {
                cancelled(url)?;
                if (self.draining)() {
                    return Ok(false);
                }
                let nap = poll.min(*left);
                std::thread::sleep(nap);
                *left -= nap;
                if !refused() {
                    eprintln!("fetch: {url}: this Mac's agent is back; asking again");
                    return Ok(true);
                }
            }
            Ok(false)
        }

        /// A request's whole wait for its proxy (`proxy_back`).
        fn proxy_wait(&self) -> Duration {
            self.second * PROXY_WAIT_S as u32
        }

        /// With the notes of hosts that gave no answer in `dir` (super::down_check) in place of
        /// SCENIC_FETCH_DOWN's.
        #[cfg(test)]
        pub fn with_down(self, dir: Option<PathBuf>) -> Net {
            Net { down: dir, ..self }
        }

        /// With the retries' waits counted in `second`s (a test's few ms): the same tries in
        /// the same order, without waiting out real seconds.
        #[cfg(test)]
        pub fn with_second(self, second: Duration) -> Net {
            Net { second, ..self }
        }

        /// HTTPS only, and no redirect followed (one comes back as an answer, which isn't a file:
        /// an error).
        pub fn strict() -> Net {
            Net {
                agent: Self::config().https_only(true).max_redirects(0).build().into(),
                down: down_dir(),
                second: Duration::from_secs(1),
                proxy: local_proxy(),
                draining: crate::control::draining_lately,
            }
        }

        fn config() -> ureq::config::ConfigBuilder<ureq::typestate::AgentScope> {
            ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(90)))
                .timeout_connect(Some(Duration::from_secs(30)))
                .user_agent(USER_AGENT)
                .accept_encoding("identity")
                .http_status_as_error(false)
                .max_idle_connections(256)
                .max_idle_connections_per_host(64)
        }

        /// `url`'s file: from the mirror's part of it (`local`) where that has the bytes, else
        /// fetched, and recorded in `record`; None when the server has no such file.
        pub fn open(
            &self,
            url: &str,
            local: Option<Local>,
            record: Option<&Path>,
        ) -> Result<Option<Arc<dyn RangeRead>>> {
            let (len, first) = match &local {
                Some(l) => (l.len, Vec::new()),
                None => match self.head(url)? {
                    Some(h) => h,
                    None => {
                        if let Some(r) = record {
                            Recorder::none(r, url).ok();
                        }
                        return Ok(None);
                    }
                },
            };
            let record = record.map(|r| Recorder::new(r, url, len)).transpose()?;
            if let Some(r) = &record {
                if !first.is_empty() {
                    r.range(0, &first).ok();
                }
            }
            let mut blocks = Blocks::default();
            for (i, chunk) in first.chunks(BLOCK as usize).enumerate() {
                // (Only whole blocks, and the file's last.)
                if chunk.len() as u64 == BLOCK || (i as u64 * BLOCK + chunk.len() as u64) == len {
                    blocks.put(i as u64, Arc::from(chunk));
                }
            }
            Ok(Some(Arc::new(Remote {
                url: url.to_string(),
                len,
                local,
                online: Some(Online { net: self.clone(), record, blocks: Mutex::new(blocks) }),
            })))
        }

        /// `url`'s small file whole (`get`), recorded in `record`; each request held to
        /// `deadline` when one's given (`Fetch::get_within`).
        pub fn get_recorded(
            &self,
            url: &str,
            record: Option<&Path>,
            deadline: Option<super::Within>,
        ) -> Result<Option<Vec<u8>>> {
            let got = self.get_by(url, deadline)?;
            if let Some(r) = record {
                let w = match &got {
                    Some(b) => Recorder::whole(r, url, b),
                    None => Recorder::none(r, url),
                };
                if let Err(e) = w {
                    eprintln!("fetch: {url}: not recorded: {e:#}");
                }
            }
            Ok(got)
        }

        /// Asks again after a transient failure: 10 times, waiting 2 s, then twice as long each
        /// time (at most a minute); not at all while its host's note says it gave no answer lately.
        fn retrying<T>(&self, url: &str, mut f: impl FnMut() -> Result<std::result::Result<T, String>>) -> Result<T> {
            down_check(self.down.as_deref(), url)?;
            let mut last = String::new();
            let mut attempt = 0u32;
            let mut proxy_left = self.proxy_wait();
            while attempt <= 10 {
                cancelled(url)?;
                match f()? {
                    Ok(v) => return Ok(v),
                    Err(e) => last = e,
                }
                if self.proxy_back(url, &mut proxy_left)? {
                    continue;
                }
                attempt += 1;
                if attempt <= 10 {
                    retry_wait(self.down.as_deref(), url, (2u64 << (attempt - 1)).min(60), self.second)?;
                }
            }
            note_down(self.down.as_deref(), url, &last);
            bail!("{url}{NO_ANSWER}{last}); tried 11 times")
        }

        /// The file's length and first bytes; None when the server has no such file.
        fn head(&self, url: &str) -> Result<Option<(u64, Vec<u8>)>> {
            let _slot = HostSlot::take(url);
            self.retrying(url, || {
                crate::fetch::online(url)?;
                let mut r = match self.agent.get(url).header("Range", format!("bytes=0-{}", READ_AHEAD - 1)).call() {
                    Ok(r) => r,
                    Err(e) => return Ok(Err(e.to_string())),
                };
                match r.status().as_u16() {
                    206 => {
                        let Some((Some((0, _)), Some(total))) =
                            header(&r, "content-range").as_deref().and_then(content_range)
                        else {
                            bail!("{url}: a range answer without its length");
                        };
                        match r.body_mut().with_config().limit(READ_AHEAD + 1).read_to_vec() {
                            Ok(b) if b.len() as u64 == READ_AHEAD.min(total) => {
                                fetched(b.len());
                                Ok(Ok(Some((total, b))))
                            }
                            Ok(b) => Ok(Err(format!("{} of {} bytes", b.len(), READ_AHEAD.min(total)))),
                            Err(e) => Ok(Err(e.to_string())),
                        }
                    }
                    200 => {
                        // The whole file (a server that ignores ranges): only a small one is read.
                        let n: Option<u64> = header(&r, "content-length").and_then(|v| v.parse().ok());
                        ensure!(n.is_some_and(|n| n <= 64 << 20), "{url}: the server doesn't serve byte ranges");
                        match r.body_mut().with_config().limit(64 << 20).read_to_vec() {
                            Ok(b) if Some(b.len() as u64) == n => Ok(Ok(Some((b.len() as u64, b)))),
                            Ok(b) => Ok(Err(format!("{} of {n:?} bytes", b.len()))),
                            Err(e) => Ok(Err(e.to_string())),
                        }
                    }
                    416 => Ok(Ok(Some((0, Vec::new())))),
                    403 | 404 | 410 => Ok(Ok(None)),
                    c if transient(c) => Ok(Err(format!("status {c}"))),
                    c => bail!("{url}: status {c}"),
                }
            })
        }

        /// Bytes `s..e` of the file, streamed: a transfer that breaks off (the server closing the
        /// connection, a stall) goes on from the bytes received, with a new range request, after
        /// 2 s; only a request that moves nothing counts against the retries (as `retrying`: 10
        /// more, waiting 2 s, then twice as long each time, at most a minute), and a hundred
        /// requests in all end it. So a zip's member of tens of MB from a server that drops
        /// connections is never fetched again from its start. A resumed request says which file it
        /// goes on with (`If-Range`: the first answer's ETag, else its Last-Modified) and the bytes
        /// are checked against the first answer's length: a file changed meanwhile (the server
        /// answering it whole, 200, or another validator or length) is fetched again from the start.
        pub(super) fn range(&self, url: &str, s: u64, e: u64) -> Result<Vec<u8>> {
            const CHUNK: usize = 1 << 20;
            let _slot = HostSlot::take(url);
            let want = (e - s) as usize;
            let mut out: Vec<u8> = Vec::with_capacity(want);
            let (mut idle, mut requests) = (0u32, 0u32);
            let mut proxy_left = self.proxy_wait();
            // (The first answer's validator and the file's length, which every later one must match.)
            let mut first: Option<(Option<String>, Option<u64>)> = None;
            down_check(self.down.as_deref(), url)?;
            loop {
                if requests > 0 {
                    let secs = if idle == 0 { 2 } else { (2u64 << (idle - 1)).min(60) };
                    retry_wait(self.down.as_deref(), url, secs, self.second)?;
                }
                cancelled(url)?;
                crate::fetch::online(url)?;
                requests += 1;
                let from = s + out.len() as u64;
                let before = out.len();
                let r = (|| -> Result<std::result::Result<(), String>> {
                    let mut req = self.agent.get(url).header("Range", format!("bytes={from}-{}", e - 1));
                    if let Some((Some(v), _)) = &first {
                        req = req.header("If-Range", v);
                    }
                    let mut r = match req.call() {
                        Ok(r) => r,
                        Err(err) => return Ok(Err(err.to_string())),
                    };
                    match r.status().as_u16() {
                        206 => {
                            let cr = header(&r, "content-range").as_deref().and_then(content_range);
                            ensure!(
                                matches!(cr, Some((Some((a, b)), _)) if a == from && b == e - 1),
                                "{url}: asked for bytes {from}-{}, answered {cr:?}",
                                e - 1
                            );
                            let total = cr.and_then(|(_, t)| t);
                            let validator = header(&r, "etag").or_else(|| header(&r, "last-modified"));
                            match &first {
                                None => first = Some((validator, total)),
                                Some((v, t)) if *v != validator || *t != total => {
                                    return Ok(Err("the file changed while it was read".into()))
                                }
                                Some(_) => {}
                            }
                            let mut rd = r.body_mut().with_config().limit(e - from + 1).reader();
                            let mut buf = vec![0u8; CHUNK.min(want - out.len())];
                            loop {
                                match std::io::Read::read(&mut rd, &mut buf) {
                                    Ok(0) => break,
                                    Ok(n) => {
                                        ensure!(out.len() + n <= want, "{url}: more than the {want} bytes asked for");
                                        out.extend_from_slice(&buf[..n]);
                                        fetched(n);
                                    }
                                    Err(err) => return Ok(Err(err.to_string())),
                                }
                            }
                            if out.len() == want {
                                Ok(Ok(()))
                            } else {
                                Ok(Err(format!("{} of {} bytes", out.len(), want)))
                            }
                        }
                        // (The whole file: a resumed request whose If-Range no longer matches, or a
                        // server that ignores ranges; either way not what was asked for.)
                        200 if first.is_some() => Ok(Err("the file changed while it was read".into())),
                        c if transient(c) => Ok(Err(format!("status {c}"))),
                        c => bail!("{url}: bytes {from}-{}: status {c}", e - 1),
                    }
                })()?;
                let last = match r {
                    Ok(()) => return Ok(out),
                    Err(why) => why,
                };
                if last == "the file changed while it was read" {
                    // (From the start, of the file as it is now.)
                    out.clear();
                    first = None;
                    idle += 1;
                } else if out.len() > before {
                    // (It moved: on from here, after a breath.)
                    idle = 0;
                } else if self.proxy_back(url, &mut proxy_left)? {
                    // (The agent's proxy back after a restart: asked again, not counted.)
                    requests -= 1;
                } else {
                    idle += 1;
                }
                if idle > 10 {
                    note_down(self.down.as_deref(), url, &last);
                    bail!("{url}{NO_ANSWER}{last}); tried 11 times");
                }
                if requests >= 100 {
                    bail!("{url}: {} of {want} bytes after {requests} requests ({last})", out.len());
                }
            }
        }

        /// The whole (small) file; None for 404. Other answers are asked again: 8 tries, waiting 2,
        /// 4, 6 … s. Each attempt is held to `deadline` when one's given (the agent's own 90 s
        /// in all otherwise): a connection silent past its head's wait, or a body not all in by
        /// its time, is an attempt failed, asked again. A host whose every failure was a body past
        /// its time isn't noted down (`note_down`): it answers, slowly, and a busy link (many
        /// tiles at once) mustn't set the job's z6 tiles aside; one that never answered is.
        pub(super) fn get_by(&self, url: &str, deadline: Option<super::Within>) -> Result<Option<Vec<u8>>> {
            down_check(self.down.as_deref(), url)?;
            let _slot = HostSlot::take(url);
            let mut last = String::new();
            let mut all_late = true;
            // (Late: an answer that began, its body slow. A connection or a name that timed
            // out, or a head that never came, is a host giving no answer: noted down.)
            let late = |e: &ureq::Error| matches!(e, ureq::Error::Timeout(ureq::Timeout::RecvBody));
            let mut attempt = 0u64;
            let mut proxy_left = self.proxy_wait();
            while attempt < 8 {
                if attempt > 0 {
                    retry_wait(self.down.as_deref(), url, 2 * attempt, self.second)?;
                }
                attempt += 1;
                cancelled(url)?;
                crate::fetch::online(url)?;
                let req = self.agent.get(url);
                let req = match deadline {
                    // (The agent's 90 s in all lifted: the head's wait, then the body's by its size.)
                    Some(d) => req
                        .config()
                        .timeout_global(None)
                        .timeout_recv_response(Some(d.head))
                        .timeout_recv_body(Some(d.body()))
                        .build(),
                    None => req,
                };
                match req.call() {
                    Ok(mut r) => match r.status().as_u16() {
                        200 => match r.body_mut().with_config().limit(64 << 20).read_to_vec() {
                            Ok(b) => {
                                fetched(b.len());
                                return Ok(Some(b));
                            }
                            Err(e) => {
                                all_late &= late(&e);
                                last = e.to_string();
                            }
                        },
                        404 => return Ok(None),
                        c => {
                            all_late = false;
                            last = format!("status {c}");
                        }
                    },
                    Err(e) => {
                        all_late &= late(&e);
                        last = e.to_string();
                        if self.proxy_back(url, &mut proxy_left)? {
                            // (The agent's proxy back after a restart: asked again, not counted.)
                            attempt -= 1;
                            continue;
                        }
                    }
                }
            }
            if deadline.is_none() || !all_late {
                note_down(self.down.as_deref(), url, &last);
            }
            bail!("{url}{NO_ANSWER}{last})")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::net::{content_range, Recorder};
    use super::*;

    /// Every producer script that fetches (producers/, dem/: urllib, curl, producers/depth/grid.py's
    /// helpers) declares what it fetches, kinds of `Kind` (`FETCHES = "api, data"`; a shell
    /// script's `# FETCHES: data`), and one that fetches pages reads robots.txt and tells its proxy
    /// what it found (`/robots`: crate::budget::net::Robots, the Activity tab's column). (The declarations'
    /// `kind` is crate::inputs::sources::parse's.)
    #[test]
    fn every_fetching_script_declares_its_kinds() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        // (Asking only this Mac: the map's server.)
        const LOCAL: [&str; 2] = ["producers/depth/smalllakes.py", "producers/depth/smalllakes-run.sh"];
        fn walk(d: &Path, out: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                let name = e.file_name().to_string_lossy().to_string();
                if p.is_dir() && !name.starts_with('.') {
                    walk(&p, out);
                } else if (name.ends_with(".py") || name.ends_with(".sh")) && !name.starts_with("test_") {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(&root.join("producers"), &mut files);
        walk(&root.join("dem"), &mut files);
        let mut checked = 0;
        for p in files {
            let rel = p.strip_prefix(&root).unwrap().to_string_lossy().to_string();
            let text = std::fs::read_to_string(&p).unwrap();
            let fetches = text.contains("urllib.request")
                || text.contains("urlretrieve")
                || text.lines().any(|l| {
                    let l = l.trim_start();
                    !l.starts_with('#')
                        && (l.contains("\"curl\"")
                            || l.starts_with("curl ")
                            || l.contains("$(curl ")
                            || (l.starts_with("from grid import") && (l.contains("fetch") || l.contains("urlopen"))))
                });
            if !fetches || LOCAL.contains(&rel.as_str()) {
                continue;
            }
            let decl = text.lines().find_map(|l| {
                l.strip_prefix("FETCHES = \"")
                    .and_then(|r| r.strip_suffix('"'))
                    .or_else(|| l.strip_prefix("# FETCHES: "))
            });
            let decl = decl.unwrap_or_else(|| panic!("{rel} fetches but declares no FETCHES (api, data, page)"));
            let kinds: Vec<Kind> = decl
                .split(',')
                .map(|k| {
                    serde_json::from_value(serde_json::Value::String(k.trim().into()))
                        .unwrap_or_else(|_| panic!("{rel}: FETCHES names {k:?}: api, data or page"))
                })
                .collect();
            assert!(!kinds.is_empty(), "{rel}: FETCHES names nothing");
            if kinds.contains(&Kind::Page) {
                assert!(text.contains("robots.txt"), "{rel} fetches pages but reads no robots.txt");
                assert!(text.contains("/robots?host="), "{rel} fetches pages but tells its proxy no robots.txt state");
            }
            checked += 1;
        }
        assert!(checked >= 10, "only {checked} fetching scripts found");
    }

    /// A server on this Mac answering every request 503, and its URL.
    #[cfg(not(target_os = "wasi"))]
    fn always_busy() -> String {
        use std::io::{Read, Write};
        let l = store::spawn::listen("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}/a.tif", l.local_addr().unwrap().port());
        std::thread::spawn(move || loop {
            let c = store::spawn::accept(&l).map(|c| c.0);
            let Ok(mut c) = c else { return };
            let mut b = [0u8; 4096];
            let _ = c.read(&mut b);
            let _ = c.write_all(b"HTTP/1.1 503 Busy\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        });
        url
    }

    /// Task #426 review: a tile's limits are the head's wait and a body's rate, not a cap on the
    /// whole request: a body of 40 KB coming at 20 KB/s, two seconds, is fetched whole with the
    /// head given 1 s and the body 10 KB/s or more, where a 1 s cap in all failed it.
    #[cfg(not(target_os = "wasi"))]
    #[test]
    fn a_slow_but_steady_body_is_fetched_within_a_tiles_limits() {
        use std::io::{Read, Write};
        let l = store::spawn::listen("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let url = format!("http://127.0.0.1:{}/t.png", l.local_addr().unwrap().port());
        std::thread::spawn(move || {
            let mut c = loop {
                match store::spawn::accept(&l) {
                    Ok((c, _)) => break c,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(2))
                    }
                    Err(_) => return,
                }
            };
            c.set_nonblocking(false).ok();
            let mut b = [0u8; 4096];
            let _ = c.read(&mut b);
            let _ = c.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 40000\r\nConnection: close\r\n\r\n");
            for i in 0..10u8 {
                std::thread::sleep(std::time::Duration::from_millis(200));
                if c.write_all(&[i; 4000]).is_err() {
                    return;
                }
            }
        });
        let within = Within { head: std::time::Duration::from_secs(1), floor: 10_000, most: 64_000 };
        let t0 = std::time::Instant::now();
        let got = net::Net::new().with_down(None).get_by(&url, Some(within)).unwrap().unwrap();
        assert!(t0.elapsed() > std::time::Duration::from_millis(1500), "{:?}", t0.elapsed());
        assert_eq!(got.len(), 40000);
        assert!(got[..4000].iter().all(|&b| b == 0) && got[36000..].iter().all(|&b| b == 9));
    }

    /// Task #426 re-review: a host that takes the connection and never answers (each attempt's
    /// head past its wait) is noted down, as one giving no answer: only a body past its time is
    /// late without being down. (Its eight tries' waits, 56 s, in 10 ms seconds.)
    #[cfg(not(target_os = "wasi"))]
    #[test]
    fn a_host_silent_after_connecting_is_noted_down_within_a_tiles_limits() {
        // (Never accepted: the system's backlog takes the connections, and nothing answers.)
        let l = store::spawn::listen("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}/t.png", l.local_addr().unwrap().port());
        let dir = tempfile::tempdir().unwrap();
        let within = Within { head: std::time::Duration::from_millis(200), floor: 10_000, most: 64_000 };
        let net =
            net::Net::new().with_down(Some(dir.path().to_path_buf())).with_second(std::time::Duration::from_millis(10));
        let e = net.get_by(&url, Some(within)).unwrap_err();
        assert!(is_unreachable(&e), "{e:#}");
        let note = down_note(Some(dir.path()), &url).unwrap();
        assert!(note.exists(), "not noted down: {e:#}");
        drop(l);
    }

    /// Task #397 follow-up: a range read waiting out its retries ends at once when its reader
    /// stops (`cancellable`), and when another request notes its host down meanwhile
    /// (`down_check` before each attempt), not after its retries (minutes).
    #[cfg(not(target_os = "wasi"))]
    #[test]
    fn a_retrying_read_ends_when_its_reader_stops_or_its_host_is_noted_down() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let url = always_busy();
        let flag = Arc::new(AtomicBool::new(false));
        let f = flag.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            f.store(true, Ordering::Relaxed);
        });
        let t0 = std::time::Instant::now();
        let r = cancellable(&flag, || net::Net::new().with_down(None).range(&url, 0, 10));
        let e = format!("{:#}", r.unwrap_err());
        assert!(e.contains("given up"), "{e}");
        store::testkit::ended_before(
            t0,
            std::time::Duration::from_secs(60),
            "ended as its reader stopped, not after its retries (minutes)",
        );
        // (Its host noted down by another request while it waits.)
        let d = tempfile::tempdir().unwrap();
        let (dir, u) = (d.path().to_path_buf(), url.clone());
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            note_down(Some(&dir), &u, "no answer");
        });
        let t0 = std::time::Instant::now();
        let r = net::Net::new().with_down(Some(d.path().to_path_buf())).range(&url, 0, 10);
        assert!(is_unreachable(&r.unwrap_err()));
        store::testkit::ended_before(
            t0,
            std::time::Duration::from_secs(60),
            "ended as its host was noted down, not after its retries (minutes)",
        );
    }

    /// Task #397: a host's requests in flight are capped (`HOST_REQUESTS`), another host's not
    /// held up by them, and a slot given back lets a waiting request go.
    #[cfg(not(target_os = "wasi"))]
    #[test]
    fn a_hosts_requests_at_once_are_capped() {
        use super::net::{HostSlot, HOST_REQUESTS};
        let held: Vec<HostSlot> = (0..HOST_REQUESTS).map(|_| HostSlot::take("https://capped.test/a.tif")).collect();
        let _other = HostSlot::take("https://other.test/b.tif");
        let (tx, rx) = std::sync::mpsc::channel();
        let t = std::thread::spawn(move || {
            let s = HostSlot::take("https://capped.test:443/c.tif");
            tx.send(()).unwrap();
            drop(s);
        });
        let wait = std::time::Duration::from_millis(100);
        assert!(rx.recv_timeout(wait).is_err(), "a request past the cap waits");
        drop(held);
        assert!(rx.recv_timeout(store::testkit::WATCHDOG).is_ok(), "and goes once a slot is free");
        t.join().unwrap();
    }

    #[test]
    fn mirrors_and_their_ranges() {
        let d = tempfile::tempdir().unwrap();
        let url = "https://example.org/a/b.tif?x=1";
        assert_eq!(mirror_path(d.path(), url).unwrap(), d.path().join("example.org/a/b.tif"));
        assert!(mirror_path(d.path(), "https://example.org/../etc").is_err());
        // A recorded range, then reads within it and outside it (no network).
        let rec = Recorder::new(d.path(), url, 100).unwrap();
        rec.range(10, b"0123456789").unwrap();
        rec.range(20, b"abc").unwrap();
        let f = Fetcher::new(Some(d.path().to_path_buf()), None, false);
        let r = f.open(url).unwrap().unwrap();
        assert_eq!(r.len().unwrap(), 100);
        assert_eq!(r.read_at(12, 10).unwrap(), b"23456789ab");
        assert!(r.read_at(0, 5).is_err(), "not supplied");
        assert!(f.get(url).is_err(), "only part of it");
        // A whole file, and the server's "none".
        Recorder::whole(d.path(), "https://example.org/t.png", b"png").unwrap();
        assert_eq!(f.get("https://example.org/t.png").unwrap().unwrap(), b"png");
        Recorder::none(d.path(), "https://example.org/n.tif").unwrap();
        assert!(f.open("https://example.org/n.tif").unwrap().is_none());
        assert!(f.get("https://example.org/n.tif").unwrap().is_none());
        assert!(f.open("https://example.org/missing.tif").is_err(), "not supplied, offline");
        let mut r = Ranges::default();
        for (s, e) in [(5, 7), (1, 3), (3, 4), (7, 9)] {
            r.add(s, e);
        }
        assert_eq!(r.0, vec![(1, 4), (5, 9)]);
        assert!(r.covers(5, 9) && r.covers(1, 2) && !r.covers(3, 6));
    }

    /// A server's file: its bytes and ETag (swapped to play a file changed mid-transfer), and the
    /// ranges asked of it so far.
    type Served = Arc<std::sync::Mutex<(Vec<u8>, String, Vec<(usize, usize)>)>>;

    /// A server that answers range requests from `file` (with its ETag, honouring `If-Range`:
    /// a whole 200 answer when it doesn't match) but closes the connection after `cut` bytes of
    /// each of its first `drops` answers (as a server dropping a long transfer does).
    fn dropping_server(file: Served, cut: usize, drops: usize) -> String {
        use std::io::{Read, Write};
        let l = store::spawn::listen("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let url = format!("http://{}/z.zip", l.local_addr().unwrap());
        let served = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        std::thread::spawn(move || loop {
            let (mut s, _) = match store::spawn::accept(&l) {
                Ok(c) => c,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                    continue;
                }
                Err(_) => continue,
            };
            s.set_nonblocking(false).ok();
            let mut req = Vec::new();
            let mut b = [0u8; 1];
            while s.read(&mut b).is_ok_and(|n| n == 1) {
                req.push(b[0]);
                if req.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&req).to_ascii_lowercase();
            let hdr = |k: &str| text.lines().find_map(|l| l.strip_prefix(k).map(|v| v.trim().to_string()));
            let (data, etag) = {
                let f = file.lock().unwrap();
                (f.0.clone(), f.1.clone())
            };
            let range = hdr("range: bytes=").map(|r| {
                let (a, b) = r.split_once('-').unwrap();
                (a.parse::<usize>().unwrap(), b.parse::<usize>().unwrap())
            });
            if let Some(r) = range {
                file.lock().unwrap().2.push(r);
            }
            let n = served.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let whole = range.is_none() || hdr("if-range:").is_some_and(|v| v != etag.to_ascii_lowercase());
            let (a, b) = if whole { (0, data.len() - 1) } else { range.unwrap() };
            let body = &data[a..=b];
            let send = if n < drops { body.len().min(cut) } else { body.len() };
            let head = if whole {
                format!(
                    "HTTP/1.1 200 OK\r\nETag: {etag}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
            } else {
                format!("HTTP/1.1 206 Partial Content\r\nETag: {etag}\r\nContent-Range: bytes {a}-{b}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", data.len(), body.len())
            };
            s.write_all(head.as_bytes()).ok();
            s.write_all(&body[..send]).ok();
            s.flush().ok();
            drop(s);
        });
        url
    }

    fn served(data: Vec<u8>, etag: &str) -> Served {
        Arc::new(std::sync::Mutex::new((data, etag.to_string(), Vec::new())))
    }

    #[test]
    fn a_dropped_transfer_goes_on_from_where_it_stopped() {
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        // Dropped twice after 100 KB: three requests, each going on where the last stopped, so
        // no byte is asked for twice.
        let file = served(data.clone(), "\"v1\"");
        let url = dropping_server(file.clone(), 100_000, 2);
        // (Its retries' 2 s waits in 10 ms seconds.)
        let fast = || net::Net::new().with_second(std::time::Duration::from_millis(10));
        let got = fast().range(&url, 1000, 251_000).unwrap();
        assert_eq!(got, data[1000..251_000]);
        assert_eq!(file.lock().unwrap().2, [(1000, 250_999), (101_000, 250_999), (201_000, 250_999)]);
        // A whole-file read through the fetcher's own path (the same server, dropping once).
        let url = dropping_server(served(data.clone(), "\"v1\""), 50_000, 1);
        let r = fast().open(&url, None, None).unwrap().unwrap();
        assert_eq!(r.read_at(200_000, 100_000).unwrap(), data[200_000..300_000]);
    }

    #[test]
    fn a_file_changed_mid_transfer_is_fetched_again_from_its_start() {
        let v1: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let v2: Vec<u8> = (0..300_000u32).map(|i| (i % 241) as u8).collect();
        // Dropped once after 100 KB; the file swapped meanwhile: the resumed request's If-Range
        // no longer matches, the server answers the whole file (200), and the read starts again,
        // so the bytes are the new file's throughout, never a mix.
        let file = served(v1.clone(), "\"v1\"");
        let url = dropping_server(file.clone(), 100_000, 1);
        let swap = file.clone();
        let n2 = v2.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let mut f = swap.lock().unwrap();
            (f.0, f.1) = (n2, "\"v2\"".to_string());
        });
        let got = net::Net::new().range(&url, 1000, 251_000).unwrap();
        assert_eq!(got, v2[1000..251_000]);
        assert_eq!(
            file.lock().unwrap().2,
            [(1000, 250_999), (101_000, 250_999), (1000, 250_999)],
            "resumed, answered whole, started again"
        );
        // The validator the same but the length changed: fetched again from the start too.
        let file = served(v1.clone(), "\"same\"");
        let url = dropping_server(file.clone(), 100_000, 1);
        let swap = file.clone();
        let shorter: Vec<u8> = v2[..260_000].to_vec();
        let n2 = shorter.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let mut f = swap.lock().unwrap();
            (f.0, f.1) = (n2, "\"same\"".to_string());
        });
        let got = net::Net::new().range(&url, 1000, 251_000).unwrap();
        assert_eq!(got, shorter[1000..251_000]);
    }

    /// A stand-in for this Mac's agent's proxy on 127.0.0.1:`port` (0: any), answering every
    /// request (absolute-form, or after a CONNECT) `status` with "tile", counting them, until
    /// `stop` is set: then it stops listening (its port refuses, as an agent restarting).
    fn standin_proxy(
        port: u16,
        status: u16,
        asked: Arc<std::sync::atomic::AtomicUsize>,
        stop: Arc<std::sync::atomic::AtomicBool>,
    ) -> u16 {
        use std::io::{BufRead, Write};
        use std::sync::atomic::Ordering;
        let l = store::spawn::listen(("127.0.0.1", port)).unwrap();
        l.set_nonblocking(true).unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                let Ok((s, _)) = store::spawn::accept(&l) else {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    continue;
                };
                s.set_nonblocking(false).ok();
                let asked = asked.clone();
                std::thread::spawn(move || {
                    let mut r = std::io::BufReader::new(s.try_clone().unwrap());
                    let mut s = s;
                    let head = |r: &mut std::io::BufReader<std::net::TcpStream>| -> Option<String> {
                        let mut first = String::new();
                        r.read_line(&mut first).ok().filter(|n| *n > 0)?;
                        loop {
                            let mut l = String::new();
                            if r.read_line(&mut l).ok()? == 0 || l == "\r\n" {
                                return Some(first);
                            }
                        }
                    };
                    let Some(mut first) = head(&mut r) else { return };
                    if first.starts_with("CONNECT") {
                        s.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").ok();
                        let Some(f) = head(&mut r) else { return };
                        first = f;
                    }
                    if first.starts_with("GET") {
                        asked.fetch_add(1, Ordering::SeqCst);
                        let a = format!("HTTP/1.1 {status} X\r\nContent-Length: 4\r\nConnection: close\r\n\r\ntile");
                        s.write_all(a.as_bytes()).ok();
                    }
                });
            }
        });
        port
    }

    /// (The target on localhost, which tests may ask (`online`); only the proxy answers.
    /// Its retries' seconds 20 ms: the proxy's wait `PROXY_WAIT_S` is 6 s, the 8 tries' waits
    /// 1.12 s.)
    fn through(port: u16) -> net::Net {
        net::Net::new()
            .with_second(std::time::Duration::from_millis(20))
            .with_draining(|| false)
            .through(&format!("http://127.0.0.1:{port}"))
    }

    #[test]
    fn a_request_waits_for_its_agents_proxy_to_come_back() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let asked = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(true));
        // (A free port, its listener gone: refused.)
        let port = standin_proxy(0, 200, asked.clone(), stop.clone());
        store::testkit::eventually("the stand-in proxy gone", || {
            std::net::TcpStream::connect(("127.0.0.1", port)).is_err()
        });
        // Back after 3 s: longer than all the tries' waits, within the proxy's wait.
        let (a, s2) = (asked.clone(), Arc::new(AtomicBool::new(false)));
        let back = s2.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(3000)); // (sleep ok: the agent's restart, away that long)
            standin_proxy(port, 200, a, back);
        });
        let d = tempfile::tempdir().unwrap();
        let rec = d.path().join("rec");
        let url = "http://localhost:9/xyz/dem1a_png/15/29247/12276.png";
        let t = std::time::Instant::now();
        let got = through(port).get_recorded(url, Some(&rec), None).unwrap();
        assert_eq!(got.as_deref(), Some(&b"tile"[..]));
        assert!(t.elapsed() >= std::time::Duration::from_millis(2900), "{:?}", t.elapsed());
        // Asked once (nothing reached it before), and recorded.
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        assert!(rec.exists());
        // A range read and a file's head wait as well (the proxy away again, back after 3 s).
        stop.store(true, Ordering::SeqCst);
        s2.store(true, Ordering::SeqCst);
        store::testkit::eventually("the stand-in proxy gone", || {
            std::net::TcpStream::connect(("127.0.0.1", port)).is_err()
        });
        let (a, s3) = (asked.clone(), Arc::new(AtomicBool::new(false)));
        let back = s3.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(3000)); // (sleep ok: the agent's restart, away that long)
            standin_proxy(port, 200, a, back);
        });
        let e = through(port).range(url, 0, 4);
        s3.store(true, Ordering::SeqCst);
        // (The stand-in answers 200 to a range: not a range answer, an error, but one asked of it.)
        let e = e.err().unwrap();
        assert!(!format!("{e:#}").contains("refused"), "{e:#}");
        assert_eq!(asked.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_proxy_that_never_comes_back_fails_as_before() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let asked = Arc::new(AtomicUsize::new(0));
        let port = standin_proxy(0, 200, asked.clone(), Arc::new(AtomicBool::new(true)));
        store::testkit::eventually("the stand-in proxy gone", || {
            std::net::TcpStream::connect(("127.0.0.1", port)).is_err()
        });
        let url = "http://localhost:9/a.png";
        let t = std::time::Instant::now();
        let e = through(port).get_recorded(url, None, None).err().unwrap();
        assert!(is_unreachable(&e), "{e:#}");
        // (Waited out once, 6 s, then the tries as before.)
        assert!(t.elapsed() >= std::time::Duration::from_secs(6), "{:?}", t.elapsed());
        assert!(t.elapsed() < std::time::Duration::from_secs(60), "{:?}", t.elapsed()); // (bound ok: not a second wait per try)
                                                                                        // Pausing: no wait at all.
        let t = std::time::Instant::now();
        let e = through(port).with_draining(|| true).get_recorded(url, None, None).err().unwrap();
        assert!(is_unreachable(&e), "{e:#}");
        store::testkit::ended_before(t, std::time::Duration::from_secs(6), "no wait for the proxy");
        assert_eq!(asked.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_remote_hosts_failure_through_a_listening_proxy_is_retried_as_before() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let asked = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let port = standin_proxy(0, 502, asked.clone(), stop.clone());
        let t = std::time::Instant::now();
        let e = through(port).get_recorded("http://localhost:9/b.png", None, None).err().unwrap();
        stop.store(true, Ordering::SeqCst);
        assert!(is_unreachable(&e) && format!("{e:#}").contains("status 502"), "{e:#}");
        assert_eq!(asked.load(Ordering::SeqCst), 8);
        store::testkit::ended_before(t, std::time::Duration::from_secs(6), "no wait for the proxy");
    }

    #[test]
    fn content_ranges() {
        assert_eq!(content_range("bytes 0-65535/269280396583"), Some((Some((0, 65535)), Some(269280396583))));
        assert_eq!(content_range("bytes */0"), Some((None, Some(0))));
        assert_eq!(content_range("bytes 5-9/*"), Some((Some((5, 9)), None)));
        assert_eq!(content_range("items 0-1/2"), None);
    }
}
