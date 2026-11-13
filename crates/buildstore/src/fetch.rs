//! Outside data by URL: the DEM servers' files, ESA WorldCover's, GSI's tiles. A `Fetch` opens a
//! URL's file for range reads, or gets a small one whole, and tells a file the server doesn't have
//! (an answer, kept) from one it can't reach now (an error: the step fails and runs again later).
//!
//! `Fetcher` reads, in turn: a mirror folder (`<dir>/<host>/<path>`: a file, or the byte ranges of
//! one, a caller supplied, listed in `<path>.ranges`; `<path>.none` for one the server doesn't
//! have), then the network: HTTP, with the build's User-Agent, retries and a block cache, what it
//! fetches by range kept on this Mac's disk (`kept`). A mirror folder is made by the trees' runs
//! (`Noted::save`), for a later run, or a worker, to read instead.
//! What it fetches is published data (`Kind::Data`): no robots.txt is read.
//!
//!   SCENIC_FETCH_MIRROR=<dir>   read from this mirror first
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
/// sets that z6 tile aside and goes on with its others (roads::unit::SourceDown).
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

fn down_dir() -> Option<PathBuf> {
    std::env::var_os(DOWN_ENV).filter(|d| !d.is_empty()).map(PathBuf::from)
}

fn down_note(dir: Option<&Path>, url: &str) -> Option<PathBuf> {
    // (Its host and port: two servers of one host are told apart. `host_port`, the ':' a '_'.)
    let auth = url.split("://").nth(1).unwrap_or(url).split(['/', '?', '#']).next().unwrap_or("");
    let auth = auth.rsplit_once('@').map_or(auth, |h| h.1);
    let key = auth.replace(['[', ']'], "").replace(':', "_");
    let plain = !host_of(url).is_empty() && !key.contains(['/', '\\']) && !key.starts_with('.');
    plain.then(|| dir.map(|d| d.join(key))).flatten()
}

/// An error at once when `url`'s host gave no answer less than `DOWN_S` ago (its note in `dir`).
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
fn cancelled(url: &str) -> Result<()> {
    if CANCEL.with(|c| c.borrow().as_ref().is_some_and(|f| f.load(std::sync::atomic::Ordering::Relaxed))) {
        bail!("{url}: given up (its reader stopped)");
    }
    Ok(())
}

/// A retry's wait of `secs` (each `second` long: a real second but in tests), in slices, ending
/// early in an error when this thread's reader stops; then the host's note checked again
/// (`down_check`: another request may have found it down).
fn retry_wait(dir: Option<&Path>, url: &str, secs: u64, clock: &dyn store::clock::Clock) -> Result<()> {
    // (Counted by the job's proxy, for its host: jobrt::budget::net.)
    jobrt::budget::retried(url);
    let end = clock.now() + std::time::Duration::from_secs(secs);
    loop {
        cancelled(url)?;
        let now = clock.now();
        if now >= end {
            break;
        }
        clock.sleep((end - now).min(std::time::Duration::from_millis(100)));
    }
    down_check(dir, url)
}

/// Notes in `dir` that `url`'s host gave no answer (`why`), for `down_check`.
fn note_down(dir: Option<&Path>, url: &str, why: &str) {
    if let Some(p) = down_note(dir, url) {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).ok();
        }
        std::fs::write(&p, why).ok();
    }
}

#[cfg(test)]
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

/// (This crate's unit tests' process is offline from its start, whichever test runs first: none
/// reaches the internet. Another crate's unit tests say so as their process starts: pipeline's
/// `TEST_START`. Never a feature's: an example built with a crate's dev-dependencies is no test.)
static OFFLINE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(cfg!(test));

/// No network for the rest of this process (`online`): a test's.
pub fn go_offline() {
    OFFLINE.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Every request names the map, as the steps' own do.
pub const USER_AGENT: &str = "scenic-roads/0.1 (personal offline map)";

/// What a fetched source is (docs/inputs.md §6.9), which decides whether its host's robots.txt is
/// read first: robots.txt is for crawlers, so only a crawl's is. An API, a data download or a page
/// read as a person would follows its provider's usage policy instead: `USER_AGENT`, the pool's
/// rate limits (jobrt::budget), a back-off on 429, the credit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A documented API: a query service, a WFS or WCS, MediaWiki's API, an ArcGIS REST service.
    Api,
    /// A published data download: a dump, an extract, a dataset's files; or a page or record read
    /// as a person would (a declared source's fetch).
    Data,
    /// Real crawling: links followed, or a host's pages fetched in bulk: its robots.txt read first.
    Crawl,
}

/// Outside data by URL.
pub trait Fetch: Send + Sync {
    /// `url`'s file for range reads; None when the server says it has none (404 or 410, or S3's
    /// 403 for a key that doesn't exist, said twice and without Retry-After: `net::says_none`).
    fn open(&self, url: &str) -> Result<Option<Arc<dyn RangeRead>>>;

    /// `url`'s file whole (a small one); None when the server says it has none (as `open`).
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
                let _w = store::waits::disk();
                l.file.read_exact_at(&mut b, off)?;
                return Ok(b);
            }
        }
        if let Some(o) = &self.online {
            return o.read(&self.url, self.len, off, len);
        }
        Err(IoError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{}: bytes {off}..+{len} weren't supplied (no network here)", self.url),
        )))
    }
}

/// Mirror folders, then the network (natively), what that fetches by range kept on this Mac's
/// disk (`kept`).
pub struct Fetcher {
    mirror: Option<PathBuf>,
    net: Option<net::Net>,
    kept: Option<kept::Kept>,
}

impl Fetcher {
    /// A fetcher reading `mirror` first, then (natively, when `online`) the network.
    pub fn new(mirror: Option<PathBuf>, online: bool) -> Fetcher {
        Fetcher { mirror, net: online.then(net::Net::new), kept: None }
    }

    /// With the ranges it fetches kept on this Mac's disk (`kept::Kept`).
    pub fn keeping(self, kept: Option<kept::Kept>) -> Fetcher {
        Fetcher { kept, ..self }
    }

    /// From SCENIC_FETCH_MIRROR and SCENIC_FETCH_OFFLINE, keeping what it
    /// fetches in the agent's cache (`SCENIC_CACHE`) under the job's room target (`kept::Kept::from_env`).
    pub fn from_env() -> Fetcher {
        let dir = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let offline = std::env::var("SCENIC_FETCH_OFFLINE").is_ok_and(|v| !v.is_empty() && v != "0");
        Fetcher::new(dir("SCENIC_FETCH_MIRROR"), !offline).keeping(kept::Kept::from_env())
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
        if let Some(n) = &self.net {
            if local.as_ref().is_none_or(|l| l.have.is_some()) {
                return n.open(url, local, self.kept.as_ref());
            }
        }
        match local {
            Some(l) => Ok(Some(Arc::new(Remote { url: url.to_string(), len: l.len, local: Some(l), online: None }))),
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
                    let _w = store::waits::disk();
                    l.file.read_exact_at(&mut b, 0)?;
                    return Ok(Some(b));
                }
                _ => {}
            }
        }
        if let Some(n) = &self.net {
            return n.get_by(url, deadline);
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

// The bytes this process has fetched over the network so far: counted in jobrt::timings (the
// runtime crate, task #916 step 3), which a phase's count reads; kept at this path.
pub use jobrt::timings::fetched_bytes;

/// `url`'s small file whole over the network, by this process's one `net::Net` (its retries, its
/// wait for this Mac's agent's proxy through a restart, its per-host slots); None when the server
/// says it has none (`net::says_none`). For a step's own downloads with no mirror (`Fetcher`'s are
/// the data readers').
pub fn get(url: &str) -> Result<Option<Vec<u8>>> {
    NET.get_by(url, None)
}

/// `url`'s (large) file streamed into file `to` (made again from its start each attempt), by this
/// process's one `net::Net`: true once it's all there and `whole(to)`; false when the server says
/// it has none (`net::says_none`) twice, a moment apart (`to` then left as it was). A body shorter
/// than its Content-Length, or not `whole`, is asked again, as a failure to write `to` is (a share
/// not taking it for a moment). `coming` is told how much has come (0–1), at most once a second.
/// The caller makes `to` whole under its own name (a temporary one, then renamed).
pub fn download(url: &str, to: &Path, whole: &dyn Fn(&Path) -> bool, coming: &dyn Fn(f64)) -> Result<bool> {
    NET.download(url, to, whole, coming)
}

/// `url`'s very large file (the OSM planet: ~95 GB) brought whole into `part`, on from the bytes
/// `part` already holds (a run stopped part way, or another URL of the same file: a mirror), by
/// this process's one `net::Net`: each request asks for the bytes past `part`'s end and appends
/// what comes, so a transfer that breaks off, a body past its time (`RESUME_BODY`) or a job run
/// again goes on where it stopped. True once `part` holds the whole file (its length the one the
/// server gives; the caller checks its content); false when the server says it has none twice, a
/// moment apart. A server answering whole (no ranges) writes `part` from its start; one whose
/// file's length changes between answers, or is shorter than `part`, starts `part` over. Only a
/// request that moves nothing counts against the retries (10, waiting 2 s, then twice as long
/// each time, at most a minute), 429 and 5xx answers among them. `coming` is told the bytes held
/// and the file's length, at most once a second.
pub fn resume(url: &str, part: &store::nasfs::NasPath, coming: &dyn Fn(u64, u64)) -> Result<bool> {
    NET.resume(url, part, coming)
}

/// How long one of `resume`'s requests receives its body before it's asked again from where it
/// got to: a connection gone quiet is noticed within it.
pub const RESUME_BODY: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// This process's one `net::Net` (`get`, `download`, `resume`).
static NET: std::sync::LazyLock<net::Net> = std::sync::LazyLock::new(net::Net::new);

/// Requests to one host this process has in flight at most, whichever readers ask (the
/// elevation sampler's pool, its reading ahead, a pyramid's tile fetches): found as TCP finds its
/// window (store::aimd, task #1096), a limiter a host, starting at `HOST_START`, up while answers
/// come near their best latency, halved on a slow answer, a 429 or a 5xx, never past the host's
/// ceiling (`host_ceiling`). A request holds its turn through its retries' waits (they count in
/// its latency, as the host's own slowness).
pub const HOST_START: usize = 2;

/// A host's ceiling: its provider's stated limit where it states one, else `HOST_DEFAULT`.
/// Amazon S3 (`*.amazonaws.com`: the Terrarium tiles, the stores read live) states 5,500 GETs a
/// second a prefix (its performance guidelines), far past `HOST_S3`. The hosts read live that
/// publish no limit (GSI's cyberjapandata.gsi.go.jp among them, task #665 review) get the
/// conservative default: the 16 a process every host had before the limits adapted (task #1096
/// review: 8 would have halved what a GSI tile fill or sampling may keep in flight).
pub fn host_ceiling(host: &str) -> usize {
    let h = host.split(':').next().unwrap_or(host);
    if h.ends_with(".amazonaws.com") {
        HOST_S3
    } else {
        HOST_DEFAULT
    }
}
pub const HOST_S3: usize = 64;
pub const HOST_DEFAULT: usize = 16;

pub mod kept;

mod net {
    //! HTTP: range requests with retries (as the Python steps' GDAL did: 10 retries, from 2 s,
    //! doubling), small files whole (as their GSI fetch did: 8 tries, 2 s more each time).

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

    /// Fetched blocks kept in memory, least recently used out first, and the runs of blocks being
    /// got now (`loading`: a reader wanting a block of one waits for it, never fetches it again).
    #[derive(Default)]
    struct Blocks {
        map: HashMap<u64, (Arc<[u8]>, u64)>,
        loading: HashMap<u64, Arc<Flight>>,
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

        /// Takes run `f`'s blocks out of `loading` (those still its own): whether any were.
        fn unload(&mut self, f: &Arc<Flight>) -> bool {
            let mut any = false;
            for i in f.a..=f.b {
                if self.loading.get(&i).is_some_and(|g| Arc::ptr_eq(g, f)) {
                    self.loading.remove(&i);
                    any = true;
                }
            }
            any
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

    /// A run of blocks (`a..=b`) being got, by whichever reader comes to it first (`got`'s
    /// initialization), the others wanting any of its blocks waiting for it: two threads never
    /// fetch the same blocks at once (task #1099: 2.8 % of a terrain-deep target's download was).
    struct Flight {
        a: u64,
        b: u64,
        got: std::sync::OnceLock<std::result::Result<Vec<Arc<[u8]>>, String>>,
    }

    /// Counts the bytes fetched over the network (ranges and whole files) for a phase's count
    /// (jobrt::timings::fetched_bytes).
    fn fetched(n: usize) {
        jobrt::timings::fetched(n);
    }

    /// A file's network side: the connection pool, the blocks fetched, and this Mac's disk's copy
    /// of the file's ranges (`kept`: None without a cache, or for a file
    /// whose server names no version of it).
    pub struct Online {
        net: Net,
        blocks: Mutex<Blocks>,
        kept: Option<super::kept::Version>,
    }

    impl Online {
        /// `len` bytes at `off` of the `size`-byte file at `url`: from the blocks in memory, else
        /// from those being got (waited for), else got (`get_run`: a run of them at once).
        pub fn read(&self, url: &str, size: u64, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
            let (first, last) = (off / BLOCK, (off + len as u64 - 1) / BLOCK);
            let nblocks = size.div_ceil(BLOCK);
            let mut have: HashMap<u64, Arc<[u8]>> = HashMap::new();
            let mut theirs: Vec<Arc<Flight>> = Vec::new();
            let mut mine: Vec<Arc<Flight>> = Vec::new();
            {
                let mut c = self.blocks.lock().unwrap();
                let mut runs: Vec<(u64, u64)> = Vec::new();
                for i in first..=last {
                    if let Some(b) = c.get(i) {
                        have.insert(i, b);
                    } else if let Some(f) = c.loading.get(&i) {
                        if !theirs.iter().any(|t| Arc::ptr_eq(t, f)) {
                            theirs.push(f.clone());
                        }
                    } else {
                        match runs.last_mut() {
                            Some(r) if r.1 + 1 == i => r.1 = i,
                            _ => runs.push((i, i)),
                        }
                    }
                }
                // A small read takes the next blocks along (a file's directories, a tile list).
                if len as u64 <= READ_AHEAD {
                    if let Some(r) = runs.last_mut() {
                        if r.1 == last {
                            let want = (r.0 + READ_AHEAD / BLOCK - 1).min(nblocks - 1);
                            let free = |c: &Blocks, i: u64| !c.map.contains_key(&i) && !c.loading.contains_key(&i);
                            while r.1 < want && free(&c, r.1 + 1) {
                                r.1 += 1;
                            }
                        }
                    }
                }
                for (a, b) in runs {
                    let f = Arc::new(Flight { a, b, got: Default::default() });
                    for i in a..=b {
                        c.loading.insert(i, f.clone());
                    }
                    mine.push(f);
                }
            }
            // (Its own runs first, then the others': a reader never waits while a run of its own
            // is still to be got, so no two wait on each other: getting a run waits on no other.)
            for f in mine.iter().chain(&theirs) {
                // (A run is got by whichever reader comes to it first: its own reader, or another
                // wanting it before that reader reached it.)
                let mut ran = false;
                let got = f.got.get_or_init(|| {
                    ran = true;
                    self.get_run(url, size, f.a, f.b)
                });
                // (Its blocks taken out of `loading` once, by the first reader to see it got, and,
                // got, kept in memory.)
                let mut c = self.blocks.lock().unwrap();
                let settled = c.unload(f);
                match got {
                    Ok(blocks) => {
                        for (i, block) in (f.a..).zip(blocks) {
                            if (first..=last).contains(&i) {
                                have.insert(i, block.clone());
                            }
                            if settled {
                                c.put(i, block.clone());
                            }
                        }
                    }
                    // (Its own failure: the reader's error. The mine it hasn't got yet are left
                    // to whoever wants them: out of `loading`, a later reader gets them anew.)
                    Err(e) if ran => {
                        for g in &mine {
                            c.unload(g);
                        }
                        return Err(IoError::Io(std::io::Error::other(e.clone())));
                    }
                    // (Another reader's failure: it may have stopped reading (a read ahead given
                    // up, `cancellable`), so this one asks again, as its own: it's out of
                    // `loading` now.)
                    Err(_) => {
                        for g in &mine {
                            c.unload(g);
                        }
                        drop(c);
                        return self.read(url, size, off, len);
                    }
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

        /// Blocks `a..=b` of the `size`-byte file at `url`: those this Mac's disk keeps read there
        /// (`kept`), the others fetched (each run of them in one request) and kept there.
        fn get_run(&self, url: &str, size: u64, a: u64, b: u64) -> std::result::Result<Vec<Arc<[u8]>>, String> {
            let span = |i: u64| (i * BLOCK, ((i + 1) * BLOCK).min(size));
            let on_disk = |i: u64| self.kept.as_ref().is_some_and(|k| k.has(span(i).0, span(i).1));
            if let Some(k) = &self.kept {
                k.refresh();
            }
            let mut out: Vec<Arc<[u8]>> = Vec::with_capacity((b - a + 1) as usize);
            let mut i = a;
            while i <= b {
                if let Some((s, bytes)) = self.kept.as_ref().and_then(|k| k.read(span(i).0, span(i).1)) {
                    // (The disk's run holding block i: its blocks up to b.)
                    while i <= b && span(i).1 <= s + bytes.len() as u64 {
                        let (bs, be) = span(i);
                        out.push(Arc::from(&bytes[(bs - s) as usize..(be - s) as usize]));
                        i += 1;
                    }
                    continue;
                }
                // (Fetched: the blocks up to b, or to the next one the disk has.)
                let mut j = i;
                while j < b && !on_disk(j + 1) {
                    j += 1;
                }
                let (s, e) = (span(i).0, span(j).1);
                let (bytes, version) = self.net.range(url, s, e).map_err(|e| format!("{e:#}"))?;
                if let Some(k) = &self.kept {
                    k.keep(s, &bytes, version.as_deref());
                }
                out.extend(bytes.chunks(BLOCK as usize).map(Arc::from));
                i = j + 1;
            }
            Ok(out)
        }
    }

    /// The limiters by host (one a host this process has asked, kept for its life: a few).
    static HOSTS: std::sync::LazyLock<Mutex<HashMap<String, &'static store::aimd::Limiter>>> =
        std::sync::LazyLock::new(Default::default);

    /// A request's turn of its host's limit (store::aimd), given back when dropped: its latency a
    /// sample (`read`), unless it said the host is overloaded (`congested`: a 429, a 5xx) or isn't
    /// to be measured (`unmeasured`: a whole download).
    pub(super) struct HostSlot {
        turn: store::aimd::Slot<'static>,
        congested: std::sync::atomic::AtomicBool,
        bytes: std::sync::atomic::AtomicU64,
    }

    impl HostSlot {
        /// A turn of `url`'s host, waiting while its limit's reached.
        pub(super) fn take(url: &str) -> HostSlot {
            let host = host_of(url).to_string();
            let l: &'static store::aimd::Limiter =
                *HOSTS.lock().unwrap_or_else(|e| e.into_inner()).entry(host.clone()).or_insert_with(|| {
                    Box::leak(Box::new(store::aimd::Limiter::new(
                        HOST_START,
                        host_ceiling(&host),
                        store::aimd::NET_LEAST,
                        // (No cache of this Mac's answers for a host: every answer is a sample.)
                        std::time::Duration::ZERO,
                    )))
                });
            HostSlot { turn: l.take(), congested: Default::default(), bytes: Default::default() }
        }

        /// The host answered as an overloaded one does (a 429, a 5xx).
        pub(super) fn congested(&self) {
            self.congested.store(true, std::sync::atomic::Ordering::Relaxed);
        }

        /// The request got `n` bytes.
        pub(super) fn read(&self, n: usize) {
            self.bytes.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
        }

        /// No sample of the host's latency (a whole download: its time is its length's).
        pub(super) fn unmeasured(&mut self) {
            self.turn.unmeasured();
        }

        /// The host's limit now and its requests in flight (tests).
        #[cfg(test)]
        pub(super) fn host_now(url: &str) -> (f64, usize) {
            HOSTS.lock().unwrap().get(host_of(url)).map_or((HOST_START as f64, 0), |l| l.now())
        }
    }

    impl Drop for HostSlot {
        fn drop(&mut self) {
            if self.congested.load(std::sync::atomic::Ordering::Relaxed) {
                self.turn.congested();
            }
            self.turn.read(self.bytes.load(std::sync::atomic::Ordering::Relaxed));
        }
    }

    #[derive(Clone)]
    pub struct Net {
        agent: ureq::Agent,
        /// The notes of hosts that gave no answer lately (SCENIC_FETCH_DOWN: super::down_check).
        down: Option<PathBuf>,
        /// The clock the retries and the proxy's wait sleep on: the real one but in tests
        /// (`with_clock`).
        clock: Arc<dyn store::clock::Clock>,
        /// This Mac's agent's proxy (`host:port`) that requests go through, when they do
        /// (the job's HTTPS_PROXY, a loopback address: `local_proxy`), for `proxy_back`.
        proxy: Option<String>,
        /// Whether the job is pausing (`control::draining_lately`, but in tests).
        draining: fn() -> bool,
    }

    /// How long a request waits for its Mac's agent's proxy to listen again (the agent restarting
    /// into a new app after a publish) before failing as any other.
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

    /// Whether an answer says the file isn't there. 404 and 410 say so at once; a 403 (S3's
    /// answer for a key that isn't there) only the second time for this file (`forbidden` counts
    /// them: one 403 may be a host refusing for a moment), and never one with Retry-After (a host
    /// saying "later", not "none"). Every reader goes by it: `head`, `get_by`, `download`.
    fn says_none(r: &ureq::http::Response<ureq::Body>, forbidden: &mut u32) -> bool {
        match r.status().as_u16() {
            404 | 410 => true,
            403 if r.headers().get("retry-after").is_none() => {
                *forbidden += 1;
                *forbidden >= 2
            }
            _ => false,
        }
    }

    fn header(r: &ureq::http::Response<ureq::Body>, name: &str) -> Option<String> {
        r.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string)
    }

    /// The version of the file an answer is of, as its server names it: its ETag, else its
    /// Last-Modified (what `If-Range` takes).
    fn version(r: &ureq::http::Response<ureq::Body>) -> Option<String> {
        header(r, "etag").or_else(|| header(r, "last-modified"))
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
                clock: store::clock::real(),
                proxy: local_proxy(),
                draining: jobrt::control::draining_lately,
            }
        }

        /// Through the proxy at `url` (`http://127.0.0.1:<port>`) in place of the environment's.
        #[cfg(test)]
        pub fn through(self, url: &str) -> Net {
            let agent = Self::config().proxy(Some(ureq::Proxy::new(url).unwrap())).build().into();
            Net { agent, proxy: crate::nasssh::proxy_addr(url), ..self }
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
            let poll = Duration::from_millis(500);
            while !left.is_zero() {
                cancelled(url)?;
                if (self.draining)() {
                    return Ok(false);
                }
                let nap = poll.min(*left);
                self.clock.sleep(nap);
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
            Duration::from_secs(PROXY_WAIT_S)
        }

        /// With the notes of hosts that gave no answer in `dir` (super::down_check) in place of
        /// SCENIC_FETCH_DOWN's.
        #[cfg(test)]
        pub fn with_down(self, dir: Option<PathBuf>) -> Net {
            Net { down: dir, ..self }
        }

        /// With the retries' and the proxy's waits slept on `clock` (a test's virtual one): the
        /// same tries in the same order, without waiting out real seconds.
        #[cfg(test)]
        pub fn with_clock(self, clock: Arc<dyn store::clock::Clock>) -> Net {
            Net { clock, ..self }
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
        /// fetched; None when the server has no such file. What's
        /// fetched is kept on this Mac's disk too (`kept`), by the file's version as its server
        /// names it (none named: not kept).
        pub fn open(
            &self,
            url: &str,
            local: Option<Local>,
            kept: Option<&super::kept::Kept>,
        ) -> Result<Option<Arc<dyn RangeRead>>> {
            let (len, first, version) = match &local {
                Some(l) => (l.len, Vec::new(), None),
                None => match self.head(url)? {
                    Some(h) => h,
                    None => return Ok(None),
                },
            };
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
                online: Some(Online {
                    net: self.clone(),
                    blocks: Mutex::new(blocks),
                    kept: kept.zip(version).and_then(|(k, v)| k.version(url, len, &v)),
                }),
            })))
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
                    retry_wait(self.down.as_deref(), url, (2u64 << (attempt - 1)).min(60), &*self.clock)?;
                }
            }
            note_down(self.down.as_deref(), url, &last);
            bail!("{url}{NO_ANSWER}{last}); tried 11 times")
        }

        /// The file's length, its first bytes and its version as its server names it (`version`);
        /// None when the server has no such file.
        fn head(&self, url: &str) -> Result<Option<(u64, Vec<u8>, Option<String>)>> {
            let _w = store::waits::net();
            let slot = HostSlot::take(url);
            let mut forbidden = 0;
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
                                slot.read(b.len());
                                Ok(Ok(Some((total, b, version(&r)))))
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
                            // (Held whole in memory: no version needed to keep its ranges.)
                            Ok(b) if Some(b.len() as u64) == n => Ok(Ok(Some((b.len() as u64, b, None)))),
                            Ok(b) => Ok(Err(format!("{} of {n:?} bytes", b.len()))),
                            Err(e) => Ok(Err(e.to_string())),
                        }
                    }
                    416 => Ok(Ok(Some((0, Vec::new(), None)))),
                    _ if says_none(&r, &mut forbidden) => Ok(Ok(None)),
                    403 => Ok(Err("status 403".into())),
                    c if transient(c) => {
                        slot.congested();
                        Ok(Err(format!("status {c}")))
                    }
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
        /// With the version the bytes are of (`version`; None: the server names none).
        pub(super) fn range(&self, url: &str, s: u64, e: u64) -> Result<(Vec<u8>, Option<String>)> {
            const CHUNK: usize = 1 << 20;
            let _w = store::waits::net();
            let slot = HostSlot::take(url);
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
                    retry_wait(self.down.as_deref(), url, secs, &*self.clock)?;
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
                            let validator = version(&r);
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
                        c if transient(c) => {
                            slot.congested();
                            Ok(Err(format!("status {c}")))
                        }
                        c => bail!("{url}: bytes {from}-{}: status {c}", e - 1),
                    }
                })()?;
                let last = match r {
                    Ok(()) => {
                        // (Its bytes and time a sample of the host's latency: a range request's
                        // answer is the host's, as a tile's is.)
                        slot.read(out.len());
                        return Ok((out, first.and_then(|f| f.0)));
                    }
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

        /// `super::download`: through `retrying` (its tries, its wait for this Mac's agent's
        /// proxy); each request's head within a minute, its body within two hours (a square of
        /// 2 GB comes at 0.3 MB/s too).
        pub(super) fn download(
            &self,
            url: &str,
            to: &Path,
            whole: &dyn Fn(&Path) -> bool,
            coming: &dyn Fn(f64),
        ) -> Result<bool> {
            use std::io::Read;
            let _w = store::waits::net();
            let mut slot = HostSlot::take(url);
            slot.unmeasured();
            let (mut missing, mut forbidden) = (0, 0);
            self.retrying(url, || {
                crate::fetch::online(url)?;
                let req = self
                    .agent
                    .get(url)
                    .config()
                    .timeout_global(None)
                    .timeout_recv_response(Some(Duration::from_secs(60)))
                    .timeout_recv_body(Some(Duration::from_secs(7200)))
                    .build();
                let mut r = match req.call() {
                    Ok(r) => r,
                    Err(e) => return Ok(Err(e.to_string())),
                };
                // (None only when said twice, a moment apart, a 404 too: a square of GBs isn't
                // given up on one answer. `says_none`'s 403 rules hold.)
                match r.status().as_u16() {
                    200 => {}
                    c if says_none(&r, &mut forbidden) && {
                        missing += 1;
                        missing == 2 || c == 403
                    } =>
                    {
                        return Ok(Ok(false))
                    }
                    c => return Ok(Err(format!("status {c}, asked again"))),
                }
                let want: Option<u64> = header(&r, "content-length").and_then(|v| v.parse().ok());
                let on_disk = |e: std::io::Error| Ok(Err(format!("{}: {e}", to.display())));
                let mut f = match std::fs::File::create(to) {
                    Ok(f) => f,
                    Err(e) => return on_disk(e),
                };
                let mut body = r.body_mut().with_config().limit(u64::MAX).reader();
                let mut b = vec![0u8; 16 << 20];
                let (mut n, mut at) = (0u64, std::time::Instant::now());
                loop {
                    let k = match body.read(&mut b) {
                        Ok(0) => break,
                        Ok(k) => k,
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(e) => return Ok(Err(e.to_string())),
                    };
                    if let Err(e) = f.write_all(&b[..k]) {
                        return on_disk(e);
                    }
                    n += k as u64;
                    fetched(k);
                    if let Some(w) = want.filter(|&w| w > 0) {
                        if at.elapsed() >= Duration::from_secs(1) {
                            at = std::time::Instant::now();
                            coming(n as f64 / w as f64);
                        }
                    }
                }
                if let Err(e) = f.sync_all() {
                    return on_disk(e);
                }
                drop(f);
                if let Some(w) = want.filter(|&w| w != n) {
                    return Ok(Err(format!("{n} of {w} bytes")));
                }
                if !whole(to) {
                    return Ok(Err("not whole".into()));
                }
                Ok(Ok(true))
            })
        }

        /// `super::resume`.
        pub(super) fn resume(
            &self,
            url: &str,
            part: &store::nasfs::NasPath,
            coming: &dyn Fn(u64, u64),
        ) -> Result<bool> {
            use std::io::Read;
            let _w = store::waits::net();
            let mut slot = HostSlot::take(url);
            slot.unmeasured();
            down_check(self.down.as_deref(), url)?;
            let held = || match part.metadata() {
                Ok(m) => Ok(m.len()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
                Err(e) => Err(anyhow::Error::from(e).context(part.display().to_string())),
            };
            let (mut idle, mut requests, mut missing, mut forbidden) = (0u32, 0u32, 0u32, 0u32);
            let mut proxy_left = self.proxy_wait();
            // (The file's length as the first answer gave it: every later one must give the same.)
            let mut total: Option<u64> = None;
            let mut told = std::time::Instant::now();
            loop {
                if requests > 0 {
                    let secs = if idle == 0 { 2 } else { (2u64 << (idle - 1)).min(60) };
                    retry_wait(self.down.as_deref(), url, secs, &*self.clock)?;
                }
                cancelled(url)?;
                crate::fetch::online(url)?;
                requests += 1;
                let from = held()?;
                let r = (|| -> Result<std::result::Result<bool, String>> {
                    let req = self
                        .agent
                        .get(url)
                        .header("Range", format!("bytes={from}-"))
                        .config()
                        .timeout_global(None)
                        .timeout_recv_response(Some(Duration::from_secs(60)))
                        .timeout_recv_body(Some(super::RESUME_BODY))
                        .build();
                    let mut r = match req.call() {
                        Ok(r) => r,
                        Err(e) => return Ok(Err(e.to_string())),
                    };
                    let mut changed = |t: Option<u64>| -> Result<bool> {
                        let was = total.replace(t.unwrap_or(0));
                        Ok(was.is_some_and(|w| Some(w) != t))
                    };
                    let append = match r.status().as_u16() {
                        206 => {
                            let cr = header(&r, "content-range").as_deref().and_then(content_range);
                            let Some((Some((a, _)), t)) = cr else {
                                bail!("{url}: a range answer without its range ({cr:?})")
                            };
                            ensure!(a == from, "{url}: asked for bytes {from}-, answered from {a}");
                            if t.is_none() || changed(t)? {
                                part.remove_file().ok();
                                total = None;
                                return Ok(Err("the file's length changed (or wasn't given): from its start".into()));
                            }
                            true
                        }
                        // (Nothing past the bytes held: the part is the whole file, or longer than
                        // it, then started over.)
                        416 => {
                            let t = header(&r, "content-range").as_deref().and_then(content_range).and_then(|c| c.1);
                            if t == Some(from) {
                                return Ok(Ok(true));
                            }
                            part.remove_file().ok();
                            total = None;
                            return Ok(Err(format!("{from} bytes held, the file's {t:?}: from its start")));
                        }
                        // (The whole file: a server that ignores ranges.)
                        200 => {
                            let t = header(&r, "content-length").and_then(|v| v.parse().ok());
                            if t.is_none() {
                                return Ok(Err("a whole answer without its length".into()));
                            }
                            changed(t)?;
                            false
                        }
                        c if says_none(&r, &mut forbidden) && {
                            missing += 1;
                            missing == 2 || c == 403
                        } =>
                        {
                            return Ok(Ok(false))
                        }
                        c => {
                            if transient(c) {
                                slot.congested();
                            }
                            return Ok(Err(format!("status {c}")));
                        }
                    };
                    let want = total.unwrap_or(0);
                    let start = if append { from } else { 0 };
                    let mut o = std::fs::OpenOptions::new();
                    o.write(true).create(true);
                    if append {
                        o.append(true);
                    } else {
                        o.truncate(true);
                    }
                    let on_disk = |e: std::io::Error| Ok(Err(format!("{}: {e}", part.display())));
                    let mut f = match part.open_with(&o) {
                        Ok(f) => f,
                        Err(e) => return on_disk(e),
                    };
                    let mut body = r.body_mut().with_config().limit(u64::MAX).reader();
                    let mut b = vec![0u8; 16 << 20];
                    let mut n = 0u64;
                    let ended = loop {
                        let k = match body.read(&mut b) {
                            Ok(0) => break None,
                            Ok(k) => k,
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                            Err(e) => break Some(e.to_string()),
                        };
                        if let Err(e) = f.write_all(&b[..k]) {
                            break Some(format!("{}: {e}", part.display()));
                        }
                        n += k as u64;
                        fetched(k);
                        if told.elapsed() >= Duration::from_secs(1) {
                            told = std::time::Instant::now();
                            coming(start + n, want);
                        }
                    };
                    // (What came is kept, whatever ended it: the next request goes on from it.)
                    if let Err(e) = f.close() {
                        return on_disk(e);
                    }
                    coming(start + n, want);
                    match ended {
                        Some(why) => Ok(Err(why)),
                        None if start + n == want => Ok(Ok(true)),
                        None => Ok(Err(format!("{} of {want} bytes", start + n))),
                    }
                })()?;
                let last = match r {
                    Ok(done) => return Ok(done),
                    Err(why) => why,
                };
                if held()? > from {
                    // (It moved: on from here, after a breath.)
                    idle = 0;
                } else if self.proxy_back(url, &mut proxy_left)? {
                    requests -= 1;
                } else {
                    idle += 1;
                }
                if idle > 10 {
                    note_down(self.down.as_deref(), url, &last);
                    bail!("{url}{NO_ANSWER}{last}); tried 11 times");
                }
            }
        }

        /// The whole (small) file; None when the server says it has none (`says_none`, as
        /// `head`). Other answers are asked again: 8 tries, waiting 2,
        /// 4, 6 … s. Each attempt is held to `deadline` when one's given (the agent's own 90 s
        /// in all otherwise): a connection silent past its head's wait, or a body not all in by
        /// its time, is an attempt failed, asked again. A host whose every failure was a body past
        /// its time isn't noted down (`note_down`): it answers, slowly, and a busy link (many
        /// tiles at once) mustn't set the job's z6 tiles aside; one that never answered is.
        pub(super) fn get_by(&self, url: &str, deadline: Option<super::Within>) -> Result<Option<Vec<u8>>> {
            down_check(self.down.as_deref(), url)?;
            let _w = store::waits::net();
            let slot = HostSlot::take(url);
            let mut last = String::new();
            let mut all_late = true;
            // (Late: an answer that began, its body slow. A connection or a name that timed
            // out, or a head that never came, is a host giving no answer: noted down.)
            let late = |e: &ureq::Error| matches!(e, ureq::Error::Timeout(ureq::Timeout::RecvBody));
            let mut attempt = 0u64;
            let mut busy = 0u32;
            let mut forbidden = 0;
            let mut proxy_left = self.proxy_wait();
            while attempt < 8 {
                if attempt > 0 {
                    retry_wait(self.down.as_deref(), url, 2 * attempt, &*self.clock)?;
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
                                slot.read(b.len());
                                return Ok(Some(b));
                            }
                            Err(e) => {
                                all_late &= late(&e);
                                last = e.to_string();
                            }
                        },
                        _ if says_none(&r, &mut forbidden) => return Ok(None),
                        c => {
                            if transient(c) {
                                slot.congested();
                            }
                            all_late = false;
                            last = format!("status {c}");
                        }
                    },
                    // (The budgets' proxy busy past its wait (its 503 to the CONNECT; a 502 or a
                    // 400 is an attempt): no attempt spent, a minute's more waits in all.)
                    Err(ureq::Error::ConnectProxyFailed(m)) if m.contains(" 503/") && busy < 30 => {
                        all_late = false;
                        last = format!("CONNECT proxy failed: {m}");
                        busy += 1;
                        attempt -= 1;
                        self.clock.sleep(Duration::from_secs(2));
                    }
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
    use super::net::content_range;
    use super::*;

    /// Every producer script that fetches (producers/, dem/: urllib, curl, producers/depth/grid.py's
    /// helpers) declares what it fetches, kinds of `Kind` (`FETCHES = "api, data"`; a shell
    /// script's `# FETCHES: data`), and one that crawls reads robots.txt and tells its proxy
    /// what it found (`/robots`: jobrt::budget::net::Robots, the Activity tab's column). (The declarations'
    /// `kind` is inputs::inputs::sources::parse's.)
    #[test]
    fn every_fetching_script_declares_its_kinds() {
        let root = std::path::absolute("../..").unwrap();
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
            if !fetches {
                continue;
            }
            let decl = text.lines().find_map(|l| {
                l.strip_prefix("FETCHES = \"")
                    .and_then(|r| r.strip_suffix('"'))
                    .or_else(|| l.strip_prefix("# FETCHES: "))
            });
            let decl = decl.unwrap_or_else(|| panic!("{rel} fetches but declares no FETCHES (api, data, crawl)"));
            let kinds: Vec<Kind> = decl
                .split(',')
                .map(|k| {
                    serde_json::from_value(serde_json::Value::String(k.trim().into()))
                        .unwrap_or_else(|_| panic!("{rel}: FETCHES names {k:?}: api, data or crawl"))
                })
                .collect();
            assert!(!kinds.is_empty(), "{rel}: FETCHES names nothing");
            if kinds.contains(&Kind::Crawl) {
                assert!(text.contains("robots.txt"), "{rel} crawls but reads no robots.txt");
                assert!(text.contains("/robots?host="), "{rel} crawls but tells its proxy no robots.txt state");
            }
            checked += 1;
        }
        assert!(checked >= 10, "only {checked} fetching scripts found");
    }

    /// A server on this Mac answering every request 503, and its URL.
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

    /// Task #852: a small file a server answers 404 or 410 for is none at once; S3's 403 (a key
    /// that isn't there) only when said twice (one 403 may be a host refusing for a moment); a 403
    /// with Retry-After (a host saying "later") never: asked again, then an error.
    #[test]
    fn a_small_files_none_by_its_answers() {
        use std::io::{Read, Write};
        let serve = |answers: Vec<&'static str>| {
            let l = store::spawn::listen("127.0.0.1:0").unwrap();
            let url = format!("http://127.0.0.1:{}/t.png", l.local_addr().unwrap().port());
            l.set_nonblocking(true).unwrap();
            std::thread::spawn(move || {
                for a in answers {
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
                    let _ = c.write_all(a.as_bytes());
                }
            });
            url
        };
        let net = || net::Net::new().with_down(None).with_clock(store::clock::Virtual::new());
        const GONE: &str = "HTTP/1.1 410 Gone\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        const NOT_FOUND: &str = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        const FORBIDDEN: &str = "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        const LATER: &str =
            "HTTP/1.1 403 Forbidden\r\nRetry-After: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        const OK: &str = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
        for a in [GONE, NOT_FOUND] {
            assert_eq!(net().get_by(&serve(vec![a]), None).unwrap(), None, "{a}");
        }
        assert_eq!(net().get_by(&serve(vec![FORBIDDEN; 2]), None).unwrap(), None, "a 403 twice");
        let got = net().get_by(&serve(vec![FORBIDDEN, OK]), None).unwrap();
        assert_eq!(got.as_deref(), Some(&b"ok"[..]), "a 403 once, then the file");
        assert!(net().get_by(&serve(vec![LATER; 8]), None).is_err(), "a 403 with Retry-After is never none");
    }

    /// Task #426 review: a tile's limits are the head's wait and a body's rate, not a cap on the
    /// whole request: a body of 40 KB coming at 20 KB/s, two seconds, is fetched whole with the
    /// head given 1 s and the body 10 KB/s or more, where a 1 s cap in all failed it.
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
    /// late without being down. (Its eight tries' waits, 56 s, on a virtual clock.)
    #[test]
    fn a_host_silent_after_connecting_is_noted_down_within_a_tiles_limits() {
        // (Never accepted: the system's backlog takes the connections, and nothing answers.)
        let l = store::spawn::listen("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}/t.png", l.local_addr().unwrap().port());
        let dir = tempfile::tempdir().unwrap();
        let within = Within { head: std::time::Duration::from_millis(200), floor: 10_000, most: 64_000 };
        let net = net::Net::new().with_down(Some(dir.path().to_path_buf())).with_clock(store::clock::Virtual::new());
        let e = net.get_by(&url, Some(within)).unwrap_err();
        assert!(is_unreachable(&e), "{e:#}");
        let note = down_note(Some(dir.path()), &url).unwrap();
        assert!(note.exists(), "not noted down: {e:#}");
        drop(l);
    }

    /// Task #397 follow-up: a range read waiting out its retries ends at once when its reader
    /// stops (`cancellable`), and when another request notes its host down meanwhile
    /// (`down_check` before each attempt), not after its retries (minutes).
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

    /// Task #397, #1096: a host's requests in flight are held to its limit (store::aimd, starting at
    /// `HOST_START`), another host's not held up by them, and a turn given back lets a waiting
    /// request go; a 429 or 5xx halves it, answers near the best raise it, never past the host's
    /// ceiling (S3's own, the rest the conservative default).
    #[test]
    fn a_hosts_requests_at_once_are_held_to_its_limit() {
        use super::net::HostSlot;
        let url = "https://capped.test/a.tif";
        let held: Vec<HostSlot> = (0..HOST_START).map(|_| HostSlot::take(url)).collect();
        let _other = HostSlot::take("https://other.test/b.tif");
        let (tx, rx) = std::sync::mpsc::channel();
        let t = std::thread::spawn(move || {
            let s = HostSlot::take("https://capped.test:443/c.tif");
            tx.send(()).unwrap();
            drop(s);
        });
        let wait = std::time::Duration::from_millis(100);
        assert!(rx.recv_timeout(wait).is_err(), "a request past the limit waits");
        drop(held);
        assert!(rx.recv_timeout(store::testkit::WATCHDOG).is_ok(), "and goes once a turn is free");
        t.join().unwrap();
        // (Answers of a few KB, alike: up, a window at a time, to the default ceiling.)
        let url = "https://steady.test/t.png";
        for _ in 0..200 {
            let s = HostSlot::take(url);
            s.read(4096);
        }
        assert_eq!(HostSlot::host_now(url), (HOST_DEFAULT as f64, 0));
        let s = HostSlot::take(url);
        s.congested();
        drop(s);
        assert_eq!(HostSlot::host_now(url).0, HOST_DEFAULT as f64 / 2.0, "a 429 or 5xx halves it");
        assert_eq!(host_ceiling("elevation-tiles-prod.s3.amazonaws.com"), HOST_S3);
        assert_eq!(host_ceiling("s3.amazonaws.com:443"), HOST_S3);
        assert_eq!(host_ceiling("cyberjapandata.gsi.go.jp"), HOST_DEFAULT);
    }

    #[test]
    fn mirrors_and_their_ranges() {
        let d = tempfile::tempdir().unwrap();
        let url = "https://example.org/a/b.tif?x=1";
        assert_eq!(mirror_path(d.path(), url).unwrap(), d.path().join("example.org/a/b.tif"));
        assert!(mirror_path(d.path(), "https://example.org/../etc").is_err());
        // A mirror's ranges of a file (as `Noted::save` keeps them), then reads within them and
        // outside them (no network).
        let p = mirror_path(d.path(), url).unwrap();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let mut b = vec![0u8; 100];
        b[10..20].copy_from_slice(b"0123456789");
        b[20..23].copy_from_slice(b"abc");
        std::fs::write(&p, &b).unwrap();
        std::fs::write(beside(&p, ".ranges"), "10 20\n20 23\n").unwrap();
        let f = Fetcher::new(Some(d.path().to_path_buf()), false);
        let r = f.open(url).unwrap().unwrap();
        assert_eq!(r.len().unwrap(), 100);
        assert_eq!(r.read_at(12, 10).unwrap(), b"23456789ab");
        assert!(r.read_at(0, 5).is_err(), "not supplied");
        assert!(f.get(url).is_err(), "only part of it");
        // A whole file, and the server's "none".
        std::fs::write(mirror_path(d.path(), "https://example.org/t.png").unwrap(), b"png").unwrap();
        assert_eq!(f.get("https://example.org/t.png").unwrap().unwrap(), b"png");
        save_none(d.path(), "https://example.org/n.tif").unwrap();
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
        server(file, cut, drops, None)
    }

    /// `dropping_server`, each answer to a range past a file's first 64 KB (not a file's opening)
    /// held until `gate` is set, when one's given (`Gate`).
    fn server(file: Served, cut: usize, drops: usize, gate: Option<Gate>) -> String {
        use std::io::{Read, Write};
        let l = store::spawn::listen("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let url = format!("http://{}/z.zip", l.local_addr().unwrap());
        let served = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut gated = 0;
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
            let read = || {
                let f = file.lock().unwrap();
                (f.0.clone(), f.1.clone())
            };
            let (mut data, mut etag) = read();
            let raw = hdr("range: bytes=").map(|r| {
                let (a, b) = r.split_once('-').unwrap();
                (a.parse::<usize>().unwrap(), b.parse::<usize>().ok())
            });
            // (An open range, `bytes=a-`, to the file's end.)
            let to_end = |(a, b): (usize, Option<usize>), len: usize| (a, b.unwrap_or(usize::MAX).min(len.max(1) - 1));
            let mut range = raw.map(|r| to_end(r, data.len()));
            let mut cut_gated = false;
            if let Some(r) = range {
                file.lock().unwrap().2.push(r);
                if let Some(g) = gate.as_ref().filter(|_| r.0 >= READ_AHEAD_TEST) {
                    g.open.wait("the test's gate");
                    cut_gated = gated < g.cut;
                    gated += 1;
                    // (The file as it is once let through: a test may have changed it meanwhile.)
                    (data, etag) = read();
                    range = raw.map(|r| to_end(r, data.len()));
                }
            }
            let n = served.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if range.is_some_and(|r| r.0 >= data.len()) {
                let head = format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", data.len());
                s.write_all(head.as_bytes()).ok();
                continue;
            }
            let whole = range.is_none() || hdr("if-range:").is_some_and(|v| v != etag.to_ascii_lowercase());
            let (a, b) = if whole { (0, data.len() - 1) } else { range.unwrap() };
            let body = &data[a..=b];
            let send = if cut_gated {
                0
            } else if n < drops {
                body.len().min(cut)
            } else {
                body.len()
            };
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

    /// A test server's gate (`server`): its answers past a file's opening held until `open` is
    /// set, the first `cut` of them then sent without their bytes (a dropped connection).
    struct Gate {
        open: store::testkit::Signal,
        cut: usize,
    }

    fn served(data: Vec<u8>, etag: &str) -> Served {
        Arc::new(std::sync::Mutex::new((data, etag.to_string(), Vec::new())))
    }

    /// A file's opening reads its first 64 KB (`net::Net::head`).
    const READ_AHEAD_TEST: usize = 64 << 10;

    /// The ranges asked of `file` past its opening's.
    fn asked_past_head(file: &Served) -> Vec<(usize, usize)> {
        file.lock().unwrap().2.iter().copied().filter(|r| r.0 >= READ_AHEAD_TEST).collect()
    }

    #[test]
    fn blocks_being_fetched_are_waited_for_not_fetched_again() {
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let file = served(data.clone(), "\"v1\"");
        let gate = store::testkit::Signal::new();
        let url = server(file.clone(), usize::MAX, 0, Some(Gate { open: gate.clone(), cut: 0 }));
        let r = Fetcher::new(None, true).open(&url).unwrap().unwrap();
        // A reader's run of blocks asked of the server, its answer held.
        let a = {
            let r = r.clone();
            std::thread::spawn(move || r.read_at(100_000, 50_000).unwrap())
        };
        store::testkit::eventually("the first reader's request", || !asked_past_head(&file).is_empty());
        // A second reader of blocks in that run waits for it: no request of its own.
        let b = {
            let r = r.clone();
            std::thread::spawn(move || r.read_at(110_000, 10_000).unwrap())
        };
        std::thread::sleep(std::time::Duration::from_millis(300)); // (absence)
        assert_eq!(asked_past_head(&file).len(), 1, "{:?}", asked_past_head(&file));
        gate.set();
        assert_eq!(a.join().unwrap(), data[100_000..150_000]);
        assert_eq!(b.join().unwrap(), data[110_000..120_000]);
        assert_eq!(asked_past_head(&file).len(), 1, "one request for both");
    }

    #[test]
    fn a_reader_whose_run_another_failed_to_get_gets_it_itself() {
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let file = served(data.clone(), "\"v1\"");
        let gate = store::testkit::Signal::new();
        let url = server(file.clone(), usize::MAX, 0, Some(Gate { open: gate.clone(), cut: 1 }));
        let r = Fetcher::new(None, true).open(&url).unwrap().unwrap();
        // A reader reading ahead asks for a run, its answer held; a second reader waits for it.
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let a = {
            let (r, stop) = (r.clone(), stop.clone());
            std::thread::spawn(move || cancellable(&stop, || r.read_at(100_000, 50_000)))
        };
        store::testkit::eventually("the first reader's request", || !asked_past_head(&file).is_empty());
        let b = {
            let r = r.clone();
            std::thread::spawn(move || r.read_at(110_000, 10_000).unwrap())
        };
        std::thread::sleep(std::time::Duration::from_millis(300)); // (absence)
        assert_eq!(asked_past_head(&file).len(), 1, "waiting, not asking");
        // The first is stopped, its answer cut: it fails; the second asks for its own blocks.
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        gate.set();
        assert!(a.join().unwrap().is_err(), "given up");
        assert_eq!(b.join().unwrap(), data[110_000..120_000]);
        assert_eq!(asked_past_head(&file).len(), 2);
        // Nothing left being got: a later read of the first's run gets it.
        assert_eq!(r.read_at(100_000, 50_000).unwrap(), data[100_000..150_000]);
    }

    #[test]
    fn ranges_fetched_are_read_from_this_macs_disk_by_the_files_version() {
        let d = tempfile::tempdir().unwrap();
        let kept = kept::Kept::new(d.path(), 0);
        let v1: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let file = served(v1.clone(), "\"v1\"");
        let url = dropping_server(file.clone(), usize::MAX, 0);
        let fetcher = || Fetcher::new(None, true).keeping(Some(kept.clone()));
        // A job reads a range: fetched, and kept.
        let r = fetcher().open(&url).unwrap().unwrap();
        assert_eq!(r.read_at(100_000, 50_000).unwrap(), v1[100_000..150_000]);
        assert_eq!(asked_past_head(&file), [(98_304, 163_839)]);
        // A later job (a fetcher and file of its own) reads it from the disk, and fetches only
        // what the disk lacks.
        let r = fetcher().open(&url).unwrap().unwrap();
        assert_eq!(r.read_at(100_000, 50_000).unwrap(), v1[100_000..150_000]);
        assert_eq!(r.read_at(120_000, 100_000).unwrap(), v1[120_000..220_000]);
        assert_eq!(asked_past_head(&file), [(98_304, 163_839), (163_840, 229_375)]);
        // The file changed on the server: what's kept is the old one's, never read for it.
        let v2: Vec<u8> = (0..300_000u32).map(|i| (i % 241) as u8).collect();
        {
            let mut f = file.lock().unwrap();
            (f.0, f.1) = (v2.clone(), "\"v2\"".to_string());
        }
        let r = fetcher().open(&url).unwrap().unwrap();
        assert_eq!(r.read_at(100_000, 50_000).unwrap(), v2[100_000..150_000]);
        assert_eq!(asked_past_head(&file).len(), 3);
        // Without a cache (a job run by hand): fetched each time, nothing kept.
        let r = Fetcher::new(None, true).open(&url).unwrap().unwrap();
        assert_eq!(r.read_at(100_000, 50_000).unwrap(), v2[100_000..150_000]);
        assert_eq!(asked_past_head(&file).len(), 4);
    }

    #[test]
    fn a_dropped_transfer_goes_on_from_where_it_stopped() {
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        // Dropped twice after 100 KB: three requests, each going on where the last stopped, so
        // no byte is asked for twice.
        let file = served(data.clone(), "\"v1\"");
        let url = dropping_server(file.clone(), 100_000, 2);
        // (Its retries' 2 s waits on a virtual clock.)
        let fast = || net::Net::new().with_clock(store::clock::Virtual::new());
        let got = fast().range(&url, 1000, 251_000).unwrap().0;
        assert_eq!(got, data[1000..251_000]);
        assert_eq!(file.lock().unwrap().2, [(1000, 250_999), (101_000, 250_999), (201_000, 250_999)]);
        // A whole-file read through the fetcher's own path (the same server, dropping once).
        let url = dropping_server(served(data.clone(), "\"v1\""), 50_000, 1);
        let r = fast().open(&url, None, None).unwrap().unwrap();
        assert_eq!(r.read_at(200_000, 100_000).unwrap(), data[200_000..300_000]);
    }

    /// A very large file (the planet, task #1120) brought into a part file on from what it holds:
    /// a dropped transfer goes on from its end, no byte asked twice; a run stopped part way (a
    /// part already there) asks only for the rest; a part already whole asks nothing past it; one
    /// longer than the file, or a server whose file's length changes, starts it over.
    #[test]
    fn a_large_file_resumes_into_its_part_from_what_it_holds() {
        let d = tempfile::tempdir().unwrap();
        let part = store::nasfs::NasPath::local_for_test(d.path().join("planet.osm.pbf.part"));
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let fast = || net::Net::new().with_clock(store::clock::Virtual::new());
        let told = std::sync::Mutex::new(Vec::new());
        let coming = |got: u64, of: u64| told.lock().unwrap().push((got, of));
        // Dropped twice after 100 KB: three requests, each from the part's end.
        let file = served(data.clone(), "\"v1\"");
        let url = dropping_server(file.clone(), 100_000, 2);
        assert!(fast().resume(&url, &part, &coming).unwrap());
        assert_eq!(std::fs::read(part.unchecked_path()).unwrap(), data);
        assert_eq!(file.lock().unwrap().2, [(0, 299_999), (100_000, 299_999), (200_000, 299_999)]);
        assert_eq!(told.lock().unwrap().last(), Some(&(300_000, 300_000)));
        // Whole already: one request, answered 416, nothing written.
        let file = served(data.clone(), "\"v1\"");
        let url = dropping_server(file.clone(), usize::MAX, 0);
        assert!(fast().resume(&url, &part, &coming).unwrap());
        assert_eq!(file.lock().unwrap().2, [(300_000, 299_999)]);
        assert_eq!(std::fs::read(part.unchecked_path()).unwrap(), data);
        // A run stopped part way: the rest alone.
        std::fs::write(part.unchecked_path(), &data[..123_456]).unwrap();
        let file = served(data.clone(), "\"v1\"");
        let url = dropping_server(file.clone(), usize::MAX, 0);
        assert!(fast().resume(&url, &part, &coming).unwrap());
        assert_eq!(file.lock().unwrap().2, [(123_456, 299_999)]);
        assert_eq!(std::fs::read(part.unchecked_path()).unwrap(), data);
        // A part longer than the file (another file's): started over.
        let mut long = data.clone();
        long.extend_from_slice(b"more");
        std::fs::write(part.unchecked_path(), &long).unwrap();
        let file = served(data.clone(), "\"v1\"");
        let url = dropping_server(file.clone(), usize::MAX, 0);
        assert!(fast().resume(&url, &part, &coming).unwrap());
        assert_eq!(file.lock().unwrap().2, [(300_004, 299_999), (0, 299_999)]);
        assert_eq!(std::fs::read(part.unchecked_path()).unwrap(), data);
        // The file's length changed between answers (dropped after 100 KB, the next answer held
        // while the file is swapped for a shorter one): from its start, the new file whole.
        std::fs::remove_file(part.unchecked_path()).unwrap();
        let file = served(data.clone(), "\"v1\"");
        let gate = store::testkit::Signal::new();
        let url = server(file.clone(), 100_000, 1, Some(Gate { open: gate.clone(), cut: 0 }));
        let run = {
            let (url, part) = (url.clone(), part.clone());
            std::thread::spawn(move || fast().resume(&url, &part, &|_, _| {}).unwrap())
        };
        store::testkit::eventually("the second request", || file.lock().unwrap().2.len() == 2);
        let other: Vec<u8> = (0..200_000u32).map(|i| (i % 241) as u8).collect();
        {
            let mut f = file.lock().unwrap();
            (f.0, f.1) = (other.clone(), "\"v2\"".into());
        }
        gate.set();
        assert!(run.join().unwrap());
        assert_eq!(file.lock().unwrap().2, [(0, 299_999), (100_000, 299_999), (0, 199_999)]);
        assert_eq!(std::fs::read(part.unchecked_path()).unwrap(), other);
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
        assert_eq!(
            got,
            (v2[1000..251_000].to_vec(), Some("\"v2\"".to_string())),
            "the new file's bytes, and its version"
        );
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
        let got = net::Net::new().range(&url, 1000, 251_000).unwrap().0;
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
    /// Its waits on `clock`, a virtual one: what a request waited is the clock's time, the same
    /// on an idle Mac or a loaded one (task #901).)
    fn through(port: u16, clock: &Arc<store::clock::Virtual>) -> net::Net {
        net::Net::new().with_clock(clock.clone()).with_draining(|| false).through(&format!("http://127.0.0.1:{port}"))
    }

    /// The 8 tries' waits of a small file's request (`get_by`): 2, 4 … 14 s.
    const TRIES_WAIT: std::time::Duration = std::time::Duration::from_secs(56);
    /// The proxy's whole wait.
    const PROXY_WAIT: std::time::Duration = std::time::Duration::from_secs(net::PROXY_WAIT_S);

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
        // Back after 100 s: longer than all the tries' waits, within the proxy's wait.
        let back_at = std::time::Duration::from_secs(100);
        let clock = store::clock::Virtual::new();
        let (a, s2) = (asked.clone(), Arc::new(AtomicBool::new(false)));
        let back = s2.clone();
        clock.at(back_at, move || {
            standin_proxy(port, 200, a, back);
        });
        let url = "http://localhost:9/xyz/dem1a_png/15/29247/12276.png";
        let got = through(port, &clock).get_by(url, None).unwrap();
        assert_eq!(got.as_deref(), Some(&b"tile"[..]));
        assert!(clock.elapsed() >= back_at, "{:?}", clock.elapsed());
        // Asked once (nothing reached it before).
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        // A range read and a file's head wait as well (the proxy away again, back after 100 s).
        stop.store(true, Ordering::SeqCst);
        s2.store(true, Ordering::SeqCst);
        store::testkit::eventually("the stand-in proxy gone", || {
            std::net::TcpStream::connect(("127.0.0.1", port)).is_err()
        });
        let clock = store::clock::Virtual::new();
        let (a, s3) = (asked.clone(), Arc::new(AtomicBool::new(false)));
        let back = s3.clone();
        clock.at(back_at, move || {
            standin_proxy(port, 200, a, back);
        });
        let e = through(port, &clock).range(url, 0, 4);
        s3.store(true, Ordering::SeqCst);
        // (The stand-in answers 200 to a range: not a range answer, an error, but one asked of it.)
        let e = e.err().unwrap();
        assert!(!format!("{e:#}").contains("refused"), "{e:#}");
        assert!(clock.elapsed() >= back_at, "{:?}", clock.elapsed());
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
        let clock = store::clock::Virtual::new();
        let e = through(port, &clock).get_by(url, None).err().unwrap();
        assert!(is_unreachable(&e), "{e:#}");
        // (Waited out once, then the tries as before: not a second wait per try.)
        assert_eq!(clock.elapsed(), PROXY_WAIT + TRIES_WAIT);
        // Pausing: no wait for the proxy at all, the tries alone.
        let clock = store::clock::Virtual::new();
        let e = through(port, &clock).with_draining(|| true).get_by(url, None).err().unwrap();
        assert!(is_unreachable(&e), "{e:#}");
        assert_eq!(clock.elapsed(), TRIES_WAIT, "no wait for the proxy");
        assert_eq!(asked.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_remote_hosts_failure_through_a_listening_proxy_is_retried_as_before() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let asked = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let port = standin_proxy(0, 502, asked.clone(), stop.clone());
        let clock = store::clock::Virtual::new();
        let e = through(port, &clock).get_by("http://localhost:9/b.png", None).err().unwrap();
        stop.store(true, Ordering::SeqCst);
        assert!(is_unreachable(&e) && format!("{e:#}").contains("status 502"), "{e:#}");
        assert_eq!(asked.load(Ordering::SeqCst), 8);
        assert_eq!(clock.elapsed(), TRIES_WAIT, "no wait for the proxy");
    }

    #[test]
    fn content_ranges() {
        assert_eq!(content_range("bytes 0-65535/269280396583"), Some((Some((0, 65535)), Some(269280396583))));
        assert_eq!(content_range("bytes */0"), Some((None, Some(0))));
        assert_eq!(content_range("bytes 5-9/*"), Some((Some((5, 9)), None)));
        assert_eq!(content_range("items 0-1/2"), None);
    }
}
