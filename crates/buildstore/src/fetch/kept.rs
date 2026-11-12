//! What the fetcher fetches by range, kept on this Mac's disk (task #1099): read there by a later
//! read of the same bytes, in the same job or a later one here, rather than fetched again.
//!
//! Measured before it (four neighbouring terrain-deep targets over HRDEM 1 m, 2.2-2.4 GB each): 3 %
//! of a target's download was its own blocks fetched again once the fetcher's memory (16 MB a file)
//! had let them go, and each edge it shares with a neighbour 3.4 % of it; 2.1 of a target's four
//! neighbours are made in its own job, 0.5 earlier on the same Mac, 0.3 on another Mac (the lead's
//! history). So most of what's fetched twice is fetched twice on one Mac, within hours; across Macs,
//! ~1 %, isn't worth the NAS's room.
//!
//! - **Where:** the agent's cache (`SCENIC_CACHE`), `blobs/net/<host>/<path>/<length>.<hash16 of
//!   the URL and version>/<start>-<end>.<xxh3>`: one file a range fetched (block-aligned byte
//!   offsets, the end exclusive, the hash of its bytes), under the file's version as its server
//!   names it (its ETag, else its Last-Modified: `If-Range`'s validator), so bytes of one version
//!   are never read for another, nor of one URL for another (the whole URL, its query too, is in
//!   the hash). A file whose server names no version isn't kept.
//! - **Made** through store::cachefile (`put_spare`: a temporary name, named only if the name is
//!   free; never written again), only while the disk has the range's bytes free past the owner's
//!   room target (the job's, jobrt::room::job_target), as the whole copies are
//!   (crate::localread); a range answered as another version than the one the file was opened at
//!   isn't kept. Not counted as the job's disk (store::cachefile::written): a job never needs
//!   them, so they never raise what a step is granted.
//! - **Read** under the cache's shared lock (store::cachefile::read: a range read whole), its bytes
//!   checked against its name's hash (a file whose bytes never reached the disk, a Mac's crash
//!   after its naming: deleted, fetched again), marked used, its bytes counted as this Mac's
//!   disk's (store::range's `count_local`). A process lists a
//!   version's folder as it first reads past what it holds in memory, and again when the folder has
//!   changed, at most every `RELIST` (a range another process kept since then is fetched again:
//!   never wrong, only missed).
//!
//! Measured after it, the same four targets one after another on the mini (each its own process,
//! as jobs on one Mac): 8.65 GB downloaded in all, was 9.50 (−9 %), 2.12-2.22 GB a target (the first
//! 2.17 GB, its own blocks alone; each after it less its neighbours' shared edges), 236-251
//! thread-s waited on the network a target, was 288-335 (−21 %), 30 % fewer requests; 0.33-0.61 GB a
//! target read from the disk. The targets' bytes the same, and the same again with every range read
//! from the disk (the first target again: 3.0 GB read here, nothing fetched).
//! - **Freed** by room-making as `blobs/` is, each file by its own use, the least recently used first
//!   (agentcore::agent::room): a file deleted mid-job is fetched again.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// The kept ranges' folder in the agent's cache.
pub const DIR: &str = "blobs/net";

/// How often a process lists a version's folder again (when it has changed).
pub const RELIST: Duration = Duration::from_secs(10);

/// This Mac's disk's kept ranges: their folder (`DIR`, under the agent's cache), the disk's free
/// space kept past them (the owner's room target, bytes).
#[derive(Clone, Debug)]
pub struct Kept {
    dir: PathBuf,
    target: u64,
}

impl Kept {
    pub fn new(cache: &Path, target: u64) -> Kept {
        Kept { dir: cache.join(DIR), target }
    }

    /// In the agent's cache (`SCENIC_CACHE`), under the job's room target; None without a cache
    /// (a job run by hand, a test): nothing kept.
    pub fn from_env() -> Option<Kept> {
        let cache = std::env::var_os(crate::coverage::CACHE_ENV).filter(|v| !v.is_empty())?;
        Some(Kept::new(Path::new(&cache), jobrt::room::job_target()))
    }

    /// The kept ranges of the `len`-byte file at `url` as of `version` (its server's ETag or
    /// Last-Modified); None for a URL that isn't a plain path.
    pub fn version(&self, url: &str, len: u64, version: &str) -> Option<Version> {
        let dir = super::mirror_path(&self.dir, url)
            .ok()?
            .join(format!("{len}.{}", store::naming::hash16(format!("{url}\n{version}").as_bytes())));
        Some(Version {
            dir,
            version: version.to_string(),
            disk: self.dir.clone(),
            target: self.target,
            listed: Mutex::new(Listed::default()),
        })
    }
}

/// One version of a file's kept ranges (`Kept::version`).
pub struct Version {
    dir: PathBuf,
    version: String,
    /// A folder on the cache's disk that's there (for its free space).
    disk: PathBuf,
    target: u64,
    listed: Mutex<Listed>,
}

/// The ranges a process knows its version's folder holds: start → (end, the hash of its bytes), and
/// when and as of which change of the folder it was last listed.
#[derive(Default)]
struct Listed {
    runs: BTreeMap<u64, (u64, u64)>,
    at: Option<(Instant, Option<SystemTime>)>,
}

impl Version {
    /// Lists the folder again when it has changed since its last listing, at most every `RELIST`
    /// (never yet: now).
    pub fn refresh(&self) {
        let mut l = self.listed.lock().unwrap();
        if l.at.is_some_and(|(t, _)| t.elapsed() < RELIST) {
            return;
        }
        let changed = std::fs::metadata(&self.dir).and_then(|m| m.modified()).ok();
        if l.at.is_some_and(|(_, c)| c == changed) {
            l.at = Some((Instant::now(), changed));
            return;
        }
        let _w = store::waits::disk();
        let mut runs = BTreeMap::new();
        for e in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            if let Some((s, e, h)) = e.file_name().to_str().and_then(parse) {
                runs.insert(s, (e, h));
            }
        }
        // (What this process kept meanwhile stays known.)
        for (s, e) in std::mem::take(&mut l.runs) {
            runs.entry(s).or_insert(e);
        }
        *l = Listed { runs, at: Some((Instant::now(), changed)) };
    }

    /// The kept range holding bytes `s..e`, by what's listed (`refresh`): its start, end and hash.
    fn holding(&self, s: u64, e: u64) -> Option<(u64, u64, u64)> {
        let l = self.listed.lock().unwrap();
        l.runs.range(..=s).next_back().map(|(&a, &(b, h))| (a, b, h)).filter(|&(_, b, _)| b >= e)
    }

    /// Whether bytes `s..e` are kept, by what's listed.
    pub fn has(&self, s: u64, e: u64) -> bool {
        self.holding(s, e).is_some()
    }

    /// The kept range holding bytes `s..e`: its start and its bytes; None when none is (or it's
    /// gone since it was listed: forgotten; or its bytes aren't those it was named for: deleted).
    pub fn read(&self, s: u64, e: u64) -> Option<(u64, Vec<u8>)> {
        let (a, b, h) = self.holding(s, e)?;
        let p = self.dir.join(name(a, b, h));
        let got = {
            let _w = store::waits::disk();
            store::cachefile::read(&p).ok().flatten()
        };
        let whole = got.as_ref().is_some_and(|bytes| bytes.len() as u64 == b - a && store::naming::xxh3(bytes) == h);
        self.forget_unless(whole, a, &p, got.is_some());
        Some((a, got.filter(|_| whole)?))
    }

    /// When `whole`, marks kept range `p` used; else forgets it, and deletes it when it's `there`
    /// (its bytes not what it was named for).
    fn forget_unless(&self, whole: bool, a: u64, p: &Path, there: bool) {
        if whole {
            store::cachefile::touch(p);
            return;
        }
        self.listed.lock().unwrap().runs.remove(&a);
        if there {
            eprintln!("fetch: {}: not the bytes it was named for: deleted", p.display());
            store::cachefile::discard(p);
        }
    }

    /// Keeps `bytes`, fetched at `s` as `version`'s: only that of the file as it was opened, and only
    /// while the disk has them free past the room target.
    pub fn keep(&self, s: u64, bytes: &[u8], version: Option<&str>) {
        let e = s + bytes.len() as u64;
        if version != Some(self.version.as_str()) || bytes.is_empty() || self.has(s, e) {
            return;
        }
        if jobrt::room::free_past(&self.disk_there(), self.target) < bytes.len() as u64 {
            return;
        }
        let _w = store::waits::disk();
        let h = store::naming::xxh3(bytes);
        match store::cachefile::put_spare(&self.dir.join(name(s, e, h)), bytes) {
            Ok(()) => {
                self.listed.lock().unwrap().runs.entry(s).or_insert((e, h));
            }
            Err(err) => eprintln!("fetch: {}: a range not kept: {err}", self.dir.display()),
        }
    }

    /// The nearest folder above the kept ranges that's there (the disk whose room counts).
    fn disk_there(&self) -> PathBuf {
        let mut p = self.disk.as_path();
        while !p.exists() {
            match p.parent() {
                Some(q) => p = q,
                None => break,
            }
        }
        p.to_path_buf()
    }
}

/// A kept range's file name: its start, end and the hash of its bytes (store::naming::xxh3).
fn name(s: u64, e: u64, h: u64) -> String {
    format!("{s}-{e}.{h:016x}")
}

/// A kept range's start, end and hash from its file name (a temporary file's isn't one).
fn parse(n: &str) -> Option<(u64, u64, u64)> {
    let (range, h) = n.split_once('.')?;
    let (s, e) = range.split_once('-')?;
    let (s, e) = (s.parse().ok()?, e.parse().ok()?);
    let h = (h.len() == 16).then(|| u64::from_str_radix(h, 16).ok()).flatten()?;
    (s < e).then_some((s, e, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_range_is_kept_by_its_version_and_read_back() {
        let d = tempfile::tempdir().unwrap();
        let k = Kept::new(d.path(), 0);
        let v1 = k.version("https://e.org/a/f.tif", 1000, "\"v1\"").unwrap();
        v1.refresh();
        assert!(v1.read(0, 10).is_none(), "nothing kept yet");
        let bytes: Vec<u8> = (0..100u8).collect();
        v1.keep(200, &bytes, Some("\"v1\""));
        // Another version's answer isn't kept under this one.
        v1.keep(400, &bytes, Some("\"v2\""));
        assert!(v1.has(220, 300) && !v1.has(400, 410) && !v1.has(190, 210));
        assert_eq!(v1.read(250, 260), Some((200, bytes.clone())));
        // Another process (a version of its own, listing the folder) finds it; the other version
        // of the file doesn't.
        let again = k.version("https://e.org/a/f.tif", 1000, "\"v1\"").unwrap();
        again.refresh();
        assert_eq!(again.read(200, 300), Some((200, bytes.clone())));
        let v2 = k.version("https://e.org/a/f.tif", 1000, "\"v2\"").unwrap();
        v2.refresh();
        assert!(v2.read(200, 300).is_none());
        // A file deleted (room-making) is forgotten, read as not kept.
        let file = |v: &Version| {
            let f = std::fs::read_dir(&v.dir)
                .unwrap()
                .flatten()
                .find(|e| parse(&e.file_name().to_string_lossy()).is_some());
            f.unwrap().path()
        };
        std::fs::remove_file(file(&again)).unwrap();
        assert!(again.read(200, 300).is_none() && !again.has(200, 300));
        // A file whose bytes aren't those it was named for (a crash before they reached the disk:
        // zeros at its length) is deleted, read as not kept.
        v1.keep(500, &bytes, Some("\"v1\""));
        let p = file(&v1);
        std::fs::write(&p, vec![0u8; 100]).unwrap();
        assert!(v1.read(500, 600).is_none() && !v1.has(500, 600));
        assert!(!p.exists(), "deleted");
        // (A temporary file's name isn't a kept range's.)
        assert!(parse("0-100.0123456789abcdef.123.4.tmp").is_none());
        assert_eq!(parse("0-100.0123456789abcdef"), Some((0, 100, 0x0123456789abcdef)));
    }

    #[test]
    fn urls_differing_in_their_query_keep_apart() {
        let d = tempfile::tempdir().unwrap();
        let k = Kept::new(d.path(), 0);
        let a = k.version("https://e.org/f.tif?x=1", 1000, "\"v1\"").unwrap();
        a.keep(0, &[1, 2, 3], Some("\"v1\""));
        let b = k.version("https://e.org/f.tif?x=2", 1000, "\"v1\"").unwrap();
        b.refresh();
        assert!(!b.has(0, 3));
    }

    #[test]
    fn nothing_is_kept_past_the_room_target() {
        let d = tempfile::tempdir().unwrap();
        // (A target past any disk's free space.)
        let k = Kept::new(d.path(), jobrt::room::MAX_TARGET);
        let v = k.version("https://e.org/f.tif", 1000, "\"v1\"").unwrap();
        v.keep(0, &[1, 2, 3], Some("\"v1\""));
        v.refresh();
        assert!(!v.has(0, 3));
        assert!(!d.path().join(DIR).exists(), "nothing written");
    }
}
