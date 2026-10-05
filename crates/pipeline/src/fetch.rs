//! Outside data by URL: the DEM servers' files, ESA WorldCover's, GSI's tiles. A `Fetch` opens a
//! URL's file for range reads, or gets a small one whole, and tells a file the server doesn't have
//! (an answer, kept) from one it can't reach now (an error: the step fails and runs again later).
//!
//! `Fetcher` reads, in turn: a mirror folder (`<dir>/<host>/<path>`: a file, or the byte ranges of
//! one, a caller supplied, listed in `<path>.ranges`; `<path>.none` for one the server doesn't
//! have), then the network: natively HTTP, with the build's User-Agent, retries and a block cache;
//! WebAssembly has none (docs/workers.md), so there a task's caller supplies what it reads. It can
//! record what it fetched into a mirror folder, for a later run, or a worker, to read instead.
//!
//!   SCENIC_FETCH_MIRROR=<dir>   read from this mirror first
//!   SCENIC_FETCH_RECORD=<dir>   record what's fetched over the network into this mirror
//!   SCENIC_FETCH_OFFLINE=1      no network (natively too)

use anyhow::{bail, ensure, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store::range::{RangeRead, Slice};
use store::sys::PosIo;
use store::IoError;

/// Every request names the map, as the steps' own do.
pub const USER_AGENT: &str = "road-elevations/0.1 (personal offline map)";

/// Outside data by URL.
pub trait Fetch: Send + Sync {
    /// `url`'s file for range reads; None when the server says it has none (404, 403 or 410: S3's
    /// answers for a key that doesn't exist).
    fn open(&self, url: &str) -> Result<Option<Arc<dyn RangeRead>>>;

    /// `url`'s file whole (a small one); None when the server answers 404.
    fn get(&self, url: &str) -> Result<Option<Vec<u8>>>;
}

/// Byte ranges (start, end), sorted and merged.
#[derive(Clone, Debug, Default)]
struct Ranges(Vec<(u64, u64)>);

impl Ranges {
    fn add(&mut self, s: u64, e: u64) {
        if s >= e {
            return;
        }
        self.0.push((s, e));
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
                r.add(s, e);
            }
        }
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
            return Err(IoError::Io(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, format!("{}: bytes {off}..+{len} are past the end ({} bytes)", self.url, self.len))));
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
        Err(IoError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, format!("{}: bytes {off}..+{len} weren't supplied (no network here)", self.url))))
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
            return n.get_recorded(url, record.as_deref());
        }
        bail!("{url}: not supplied, and there's no network here")
    }
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

/// The member `name` of the zip that `src` holds, for range reads (a stored member is read in
/// place; a compressed one whole); None when the zip doesn't list it. Errors when its directory
/// can't be read whole.
pub fn zip_member(src: Arc<dyn RangeRead>, name: &str) -> Result<Option<Arc<dyn RangeRead>>> {
    let le16 = |b: &[u8], i: usize| u16::from_le_bytes([b[i], b[i + 1]]) as u64;
    let le32 = |b: &[u8], i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap()) as u64;
    let le64 = |b: &[u8], i: usize| u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
    let len = src.len()?;
    let tail_len = len.min(65558);
    let tail = src.read_at(len - tail_len, tail_len as usize)?;
    let i = tail.windows(4).rposition(|w| w == b"PK\x05\x06").context("no end of central directory")?;
    ensure!(tail.len() >= i + 22, "the end of central directory is cut");
    let (mut n, mut cd_size, mut cd_off) = (le16(&tail, i + 10), le32(&tail, i + 12), le32(&tail, i + 16));
    if n == 0xFFFF || cd_size == 0xFFFF_FFFF || cd_off == 0xFFFF_FFFF {
        let j = tail[..i].windows(4).rposition(|w| w == b"PK\x06\x07").context("no zip64 end locator")?;
        ensure!(i >= j + 16, "the zip64 end locator is cut");
        let rec = src.read_at(le64(&tail, j + 8), 56)?;
        ensure!(&rec[..4] == b"PK\x06\x06", "no zip64 end record");
        (n, cd_size, cd_off) = (le64(&rec, 32), le64(&rec, 40), le64(&rec, 48));
    }
    ensure!(cd_size <= 256 << 20, "a central directory of {cd_size} bytes");
    let cd = src.read_at(cd_off, cd_size as usize).context("central directory")?;
    let mut p = 0usize;
    for _ in 0..n {
        ensure!(cd.len() >= p + 46 && &cd[p..p + 4] == b"PK\x01\x02", "a damaged central directory");
        let method = le16(&cd, p + 10);
        let (mut csize, mut usize_, mut off) = (le32(&cd, p + 20), le32(&cd, p + 24), le32(&cd, p + 42));
        let (nlen, xlen, clen) = (le16(&cd, p + 28) as usize, le16(&cd, p + 30) as usize, le16(&cd, p + 32) as usize);
        ensure!(cd.len() >= p + 46 + nlen + xlen, "a damaged central directory");
        let member = String::from_utf8_lossy(&cd[p + 46..p + 46 + nlen]);
        if member == name {
            // Zip64 sizes and offset, in the extra field.
            let mut x = &cd[p + 46 + nlen..p + 46 + nlen + xlen];
            while x.len() >= 4 {
                let (id, sz) = (le16(x, 0), le16(x, 2) as usize);
                let body = &x[4..(4 + sz).min(x.len())];
                if id == 1 {
                    let mut k = 0;
                    for v in [&mut usize_, &mut csize, &mut off] {
                        if *v == 0xFFFF_FFFF && body.len() >= k + 8 {
                            *v = le64(body, k);
                            k += 8;
                        }
                    }
                }
                x = &x[(4 + sz).min(x.len())..];
            }
            let lh = src.read_at(off, 30)?;
            ensure!(&lh[..4] == b"PK\x03\x04", "{name}: no local header");
            let data = off + 30 + le16(&lh, 26) + le16(&lh, 28);
            return Ok(Some(match method {
                0 => Arc::new(Slice::new(src, data, csize)?),
                8 => {
                    ensure!(csize <= 1 << 30 && usize_ <= 1 << 31, "{name}: {csize} bytes compressed");
                    let raw = src.read_at(data, csize as usize)?;
                    let mut d = flate2::Decompress::new(false);
                    let mut out = Vec::with_capacity(usize_ as usize);
                    d.decompress_vec(&raw, &mut out, flate2::FlushDecompress::Finish).with_context(|| format!("{name}: inflate"))?;
                    ensure!(out.len() as u64 == usize_, "{name}: inflates to {} of {usize_} bytes", out.len());
                    Arc::new(out)
                }
                m => bail!("{name}: zip method {m}"),
            }));
        }
        p += 46 + nlen + xlen + clen;
    }
    Ok(None)
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
                let bytes = self.net.range(url, s, e).map_err(|e| IoError::Io(std::io::Error::other(format!("{e:#}"))))?;
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
                let b = have.get(&i).ok_or_else(|| IoError::Io(std::io::Error::other(format!("{url}: block {i} missing"))))?;
                let s = if i == first { (off - i * BLOCK) as usize } else { 0 };
                let e = if i == last { (off + len as u64 - i * BLOCK) as usize } else { b.len() };
                out.extend_from_slice(&b[s..e]);
            }
            Ok(out)
        }
    }

    #[derive(Clone)]
    pub struct Net {
        agent: ureq::Agent,
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
            Net { agent: Self::config().build().into() }
        }

        /// HTTPS only, and no redirect followed (one comes back as an answer, which isn't a file:
        /// an error).
        pub fn strict() -> Net {
            Net { agent: Self::config().https_only(true).max_redirects(0).build().into() }
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
        pub fn open(&self, url: &str, local: Option<Local>, record: Option<&Path>) -> Result<Option<Arc<dyn RangeRead>>> {
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
            Ok(Some(Arc::new(Remote { url: url.to_string(), len, local, online: Some(Online { net: self.clone(), record, blocks: Mutex::new(blocks) }) })))
        }

        /// `url`'s small file whole (`get`), recorded in `record`.
        pub fn get_recorded(&self, url: &str, record: Option<&Path>) -> Result<Option<Vec<u8>>> {
            let got = self.get(url)?;
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
        /// time (at most a minute).
        fn retrying<T>(&self, url: &str, mut f: impl FnMut() -> Result<std::result::Result<T, String>>) -> Result<T> {
            let mut last = String::new();
            for attempt in 0..=10u32 {
                if attempt > 0 {
                    std::thread::sleep(Duration::from_secs((2u64 << (attempt - 1)).min(60)));
                }
                match f()? {
                    Ok(v) => return Ok(v),
                    Err(e) => last = e,
                }
            }
            bail!("{url}: no answer ({last}); tried 11 times")
        }

        /// The file's length and first bytes; None when the server has no such file.
        fn head(&self, url: &str) -> Result<Option<(u64, Vec<u8>)>> {
            self.retrying(url, || {
                let mut r = match self.agent.get(url).header("Range", format!("bytes=0-{}", READ_AHEAD - 1)).call() {
                    Ok(r) => r,
                    Err(e) => return Ok(Err(e.to_string())),
                };
                match r.status().as_u16() {
                    206 => {
                        let Some((Some((0, _)), Some(total))) = header(&r, "content-range").as_deref().and_then(content_range) else {
                            bail!("{url}: a range answer without its length");
                        };
                        match r.body_mut().with_config().limit(READ_AHEAD + 1).read_to_vec() {
                            Ok(b) if b.len() as u64 == READ_AHEAD.min(total) => Ok(Ok(Some((total, b)))),
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

        /// Bytes `s..e` of the file.
        fn range(&self, url: &str, s: u64, e: u64) -> Result<Vec<u8>> {
            self.retrying(url, || {
                let mut r = match self.agent.get(url).header("Range", format!("bytes={s}-{}", e - 1)).call() {
                    Ok(r) => r,
                    Err(err) => return Ok(Err(err.to_string())),
                };
                match r.status().as_u16() {
                    206 => {
                        let cr = header(&r, "content-range").as_deref().and_then(content_range);
                        ensure!(matches!(cr, Some((Some((a, b)), _)) if a == s && b == e - 1), "{url}: asked for bytes {s}-{}, answered {cr:?}", e - 1);
                        match r.body_mut().with_config().limit(e - s + 1).read_to_vec() {
                            Ok(b) if b.len() as u64 == e - s => Ok(Ok(b)),
                            Ok(b) => Ok(Err(format!("{} of {} bytes", b.len(), e - s))),
                            Err(err) => Ok(Err(err.to_string())),
                        }
                    }
                    c if transient(c) => Ok(Err(format!("status {c}"))),
                    c => bail!("{url}: bytes {s}-{}: status {c}", e - 1),
                }
            })
        }

        /// The whole (small) file; None for 404. Other answers are asked again: 8 tries, waiting 2,
        /// 4, 6 … s.
        fn get(&self, url: &str) -> Result<Option<Vec<u8>>> {
            let mut last = String::new();
            for attempt in 0..8u64 {
                match self.agent.get(url).call() {
                    Ok(mut r) => match r.status().as_u16() {
                        200 => match r.body_mut().with_config().limit(64 << 20).read_to_vec() {
                            Ok(b) => return Ok(Some(b)),
                            Err(e) => last = e.to_string(),
                        },
                        404 => return Ok(None),
                        c => last = format!("status {c}"),
                    },
                    Err(e) => last = e.to_string(),
                }
                std::thread::sleep(Duration::from_secs(2 * (attempt + 1)));
            }
            bail!("{url}: no answer ({last})")
        }
    }
}

#[cfg(test)]
pub(crate) mod testzip {
    use std::io::Write;

    /// A zip of `members` (name, bytes, deflated or stored), for tests.
    pub fn zip(members: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut cd = Vec::new();
        for (name, data, deflate) in members {
            let body = if *deflate {
                let mut z = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                z.write_all(data).unwrap();
                z.finish().unwrap()
            } else {
                data.to_vec()
            };
            let off = out.len() as u32;
            let method: u16 = if *deflate { 8 } else { 0 };
            out.extend_from_slice(b"PK\x03\x04");
            out.extend_from_slice(&[20, 0, 0, 0]);
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0; 8]);
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&3u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(b"xyz");
            out.extend_from_slice(&body);
            cd.extend_from_slice(b"PK\x01\x02");
            cd.extend_from_slice(&[20, 0, 20, 0, 0, 0]);
            cd.extend_from_slice(&method.to_le_bytes());
            cd.extend_from_slice(&[0; 8]);
            cd.extend_from_slice(&(body.len() as u32).to_le_bytes());
            cd.extend_from_slice(&(data.len() as u32).to_le_bytes());
            cd.extend_from_slice(&(name.len() as u16).to_le_bytes());
            cd.extend_from_slice(&[0; 12]);
            cd.extend_from_slice(&off.to_le_bytes());
            cd.extend_from_slice(name.as_bytes());
        }
        let cd_off = out.len() as u32;
        out.extend_from_slice(&cd);
        out.extend_from_slice(b"PK\x05\x06");
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(members.len() as u16).to_le_bytes());
        out.extend_from_slice(&(members.len() as u16).to_le_bytes());
        out.extend_from_slice(&(cd.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_off.to_le_bytes());
        out.extend_from_slice(&[0; 2]);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::net::{content_range, Recorder};
    use super::testzip::zip;
    use super::*;

    #[test]
    fn zip_members_in_place() {
        let z = zip(&[("a.tif", b"stored bytes", false), ("b.tif", b"deflated bytes, deflated bytes", true)]);
        let src: Arc<dyn RangeRead> = Arc::new(z);
        let a = zip_member(src.clone(), "a.tif").unwrap().unwrap();
        assert_eq!(a.read_at(0, a.len().unwrap() as usize).unwrap(), b"stored bytes");
        let b = zip_member(src.clone(), "b.tif").unwrap().unwrap();
        assert_eq!(b.read_at(0, b.len().unwrap() as usize).unwrap(), b"deflated bytes, deflated bytes");
        assert!(zip_member(src.clone(), "c.tif").unwrap().is_none());
        assert!(zip_member(Arc::new(b"not a zip".to_vec()), "a.tif").is_err());
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

    #[test]
    fn content_ranges() {
        assert_eq!(content_range("bytes 0-65535/269280396583"), Some((Some((0, 65535)), Some(269280396583))));
        assert_eq!(content_range("bytes */0"), Some((None, Some(0))));
        assert_eq!(content_range("bytes 5-9/*"), Some((Some((5, 9)), None)));
        assert_eq!(content_range("items 0-1/2"), None);
    }
}
