//! What the views read from the NAS (a file this Mac's mirror doesn't have yet), held in memory
//! under a budget: 256 KB pages of files, for reads of a few records (a way, the steps of a binary
//! search), and whole sections, for what a query scans or a long road touches throughout. Both drop
//! the least recently used first. Mirrored files are mapped instead and never come here.

use crate::views::{Blob, RemoteFile};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;
use std::sync::{Arc, LazyLock, Mutex};
use store::iopool::IoError;

/// Bytes of a page.
pub const PAGE: u64 = 256 << 10;
const PAGES_BYTES: u64 = 384 << 20;
const WHOLES_BYTES: u64 = 1536 << 20;
/// The most a batch of range reads fetches in pages; past it the section is read whole.
pub const PAGES_PER_BATCH: u64 = PAGES_BYTES / 2;

/// A map that drops its least recently used entries past a total weight.
pub struct Lru<K, V> {
    /// Value, weight, last use.
    map: HashMap<K, (V, u64, u64)>,
    by_use: BTreeMap<u64, K>,
    tick: u64,
    bytes: u64,
    cap: u64,
}

impl<K: Hash + Eq + Clone, V: Clone> Lru<K, V> {
    pub fn new(cap: u64) -> Self {
        Lru { map: HashMap::new(), by_use: BTreeMap::new(), tick: 0, bytes: 0, cap }
    }

    pub fn get(&mut self, k: &K) -> Option<V> {
        self.tick += 1;
        let t = self.tick;
        let e = self.map.get_mut(k)?;
        self.by_use.remove(&e.2);
        e.2 = t;
        self.by_use.insert(t, k.clone());
        Some(e.0.clone())
    }

    pub fn contains(&self, k: &K) -> bool {
        self.map.contains_key(k)
    }

    /// Adds `v`; past the budget the least recently used go (never the newest).
    pub fn put(&mut self, k: K, v: V, weight: u64) {
        self.tick += 1;
        let t = self.tick;
        if let Some((_, w, used)) = self.map.insert(k.clone(), (v, weight, t)) {
            self.by_use.remove(&used);
            self.bytes -= w;
        }
        self.by_use.insert(t, k);
        self.bytes += weight;
        while self.bytes > self.cap && self.map.len() > 1 {
            let Some((_, old)) = self.by_use.pop_first() else { break };
            if let Some((_, w, _)) = self.map.remove(&old) {
                self.bytes -= w;
            }
        }
    }

    /// Keeps only the entries `keep` says.
    pub fn retain(&mut self, keep: impl Fn(&K) -> bool) {
        let gone: Vec<K> = self.map.keys().filter(|k| !keep(k)).cloned().collect();
        for k in gone {
            if let Some((_, w, used)) = self.map.remove(&k) {
                self.bytes -= w;
                self.by_use.remove(&used);
            }
        }
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

/// Pages by (file, page number); whole sections by (file, offset, length) (an empty section can
/// share its offset with the next one); sections being read whole right now, so a second request
/// waits for the first read instead of making its own.
static PAGES: LazyLock<Mutex<Lru<(u64, u64), Arc<Vec<u8>>>>> = LazyLock::new(|| Mutex::new(Lru::new(PAGES_BYTES)));
static WHOLES: LazyLock<Mutex<Lru<(u64, u64, u64), Blob>>> = LazyLock::new(|| Mutex::new(Lru::new(WHOLES_BYTES)));
static READING: LazyLock<(Mutex<std::collections::HashSet<(u64, u64, u64)>>, std::sync::Condvar)> = LazyLock::new(Default::default);

/// The page numbers bytes [off, off + len) span (len > 0).
fn span(off: u64, len: u64) -> std::ops::RangeInclusive<u64> {
    off / PAGE..=(off + len - 1) / PAGE
}

/// Page `idx` of `f`, from memory or read now.
fn page(f: &RemoteFile, idx: u64) -> Result<Arc<Vec<u8>>, IoError> {
    if let Some(p) = PAGES.lock().unwrap().get(&(f.id, idx)) {
        return Ok(p);
    }
    let start = idx * PAGE;
    let n = PAGE.min(f.len()?.saturating_sub(start)) as usize;
    let p = Arc::new(f.read_at(start, n)?);
    PAGES.lock().unwrap().put((f.id, idx), p.clone(), n as u64);
    Ok(p)
}

/// Bytes [off, off + len) of `f`, from its pages (the missing ones read in parallel).
pub fn read(f: &RemoteFile, off: u64, len: usize) -> Result<Vec<u8>, IoError> {
    if len == 0 {
        return Ok(Vec::new());
    }
    let idx: Vec<u64> = span(off, len as u64).collect();
    let pages: Vec<Arc<Vec<u8>>> = if idx.len() > 1 { idx.par_iter().map(|&i| page(f, i)).collect::<Result<_, _>>()? } else { vec![page(f, idx[0])?] };
    let end = off + len as u64;
    let mut out = Vec::with_capacity(len);
    for (&i, p) in idx.iter().zip(&pages) {
        let ps = i * PAGE;
        let (a, b) = (off.max(ps) - ps, end.min(ps + p.len() as u64).saturating_sub(ps));
        if a < b {
            out.extend_from_slice(&p[a as usize..b as usize]);
        }
    }
    if out.len() != len {
        return Err(IoError::Io(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "read past the end of the file")));
    }
    Ok(out)
}

/// The bytes of the pages that `ranges` (offset, length) span which aren't in memory.
pub fn missing(f: &RemoteFile, ranges: &[(u64, u64)]) -> u64 {
    let mut want: Vec<u64> = ranges.iter().filter(|r| r.1 > 0).flat_map(|&(o, l)| span(o, l)).collect();
    want.sort_unstable();
    want.dedup();
    let c = PAGES.lock().unwrap();
    want.iter().filter(|&&i| !c.contains(&(f.id, i))).count() as u64 * PAGE
}

/// Reads the missing pages of `ranges` (offset, length), in parallel.
pub fn prefetch(f: &RemoteFile, ranges: &[(u64, u64)]) -> Result<(), IoError> {
    let mut want: Vec<u64> = ranges.iter().filter(|r| r.1 > 0).flat_map(|&(o, l)| span(o, l)).collect();
    want.sort_unstable();
    want.dedup();
    want.retain(|&i| !PAGES.lock().unwrap().contains(&(f.id, i)));
    want.par_iter().try_for_each(|&i| page(f, i).map(|_| ()))
}

/// Bytes [off, off + len) of `f` (a section) read whole and kept: read straight into aligned memory
/// (no second copy), once however many requests want it at the same time.
pub fn whole(f: &RemoteFile, off: u64, len: u64) -> Result<Blob, IoError> {
    if len == 0 {
        return Ok(Blob::from_vec(Vec::new()));
    }
    let key = (f.id, off, len);
    let (reading, cv) = &*READING;
    {
        let mut r = reading.lock().unwrap();
        loop {
            if let Some(b) = WHOLES.lock().unwrap().get(&key) {
                return Ok(b);
            }
            if r.insert(key) {
                break;
            }
            r = cv.wait(r).unwrap();
        }
    }
    let got = f.read_aligned(off, len as usize);
    if let Ok(b) = &got {
        WHOLES.lock().unwrap().put(key, b.clone(), len);
    }
    reading.lock().unwrap().remove(&key);
    cv.notify_all();
    got
}

/// The section at [off, off + len) of `f`, if it's in memory whole.
pub fn cached_whole(f: &RemoteFile, off: u64, len: u64) -> Option<Blob> {
    WHOLES.lock().unwrap().get(&(f.id, off, len))
}

/// Keeps `b` (something made from a section, e.g. an index) under the whole sections' budget, by a
/// key of its own (`tag` past every offset).
pub fn keep_derived(f: &RemoteFile, tag: u64, b: Blob) {
    let n = b.bytes().len() as u64;
    WHOLES.lock().unwrap().put((f.id, u64::MAX - tag, 0), b, n);
}

pub fn derived(f: &RemoteFile, tag: u64) -> Option<Blob> {
    WHOLES.lock().unwrap().get(&(f.id, u64::MAX - tag, 0))
}

/// Drops what was read from the files `ids` (the mirror has them now; other NAS files keep theirs).
pub fn forget(ids: &std::collections::HashSet<u64>) {
    PAGES.lock().unwrap().retain(|k| !ids.contains(&k.0));
    WHOLES.lock().unwrap().retain(|k| !ids.contains(&k.0));
}

/// Bytes held: (pages, whole sections).
pub fn held() -> (u64, u64) {
    (PAGES.lock().unwrap().bytes(), WHOLES.lock().unwrap().bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lru_drops_the_least_recently_used() {
        let mut l: Lru<u32, u32> = Lru::new(30);
        l.put(1, 10, 10);
        l.put(2, 20, 10);
        l.put(3, 30, 10);
        assert_eq!(l.get(&1), Some(10)); // 1 is now the most recent
        l.put(4, 40, 10); // over budget: 2 goes
        assert!(l.contains(&1) && !l.contains(&2) && l.contains(&3) && l.contains(&4));
        assert_eq!(l.bytes(), 30);
        // Replacing keeps the count right; something heavier than the budget stays alone.
        l.put(3, 31, 5);
        assert_eq!(l.bytes(), 25);
        l.put(9, 90, 100);
        assert!(l.contains(&9) && l.map.len() == 1 && l.bytes() == 100);
    }

    #[test]
    fn reads_across_pages() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        let data: Vec<u8> = (0..(PAGE as usize * 3 + 100)).map(|i| (i % 251) as u8).collect();
        std::fs::write(&p, &data).unwrap();
        let pool = store::iopool::IoPool::new(2, std::time::Duration::from_secs(5), dir.path().to_owned());
        let f = RemoteFile::new(p, pool);
        for (off, len) in [(0u64, 10usize), (PAGE - 3, 6), (5, PAGE as usize * 2 + 50), (PAGE * 3, 100), (PAGE * 3 + 99, 1)] {
            assert_eq!(read(&f, off, len).unwrap(), data[off as usize..off as usize + len], "{off}+{len}");
        }
        assert!(read(&f, PAGE * 3 + 50, 51).is_err());
        assert_eq!(missing(&f, &[(0, 10), (PAGE * 2, 1)]), 0);
        let w = whole(&f, 7, 1000).unwrap();
        assert_eq!(w.bytes(), &data[7..1007]);
        assert!(cached_whole(&f, 7, 1000).is_some());
        // An empty section at the same offset as a section after it is its own (empty) thing.
        assert!(whole(&f, 7, 0).unwrap().bytes().is_empty());
        assert_eq!(whole(&f, 7, 1000).unwrap().bytes(), &data[7..1007]);
        // Forgetting a file drops its pages and sections, not other files'.
        let p2 = dir.path().join("g");
        std::fs::write(&p2, &data[..2000]).unwrap();
        let g = RemoteFile::new(p2, f.pool.clone());
        whole(&g, 0, 100).unwrap();
        forget(&[f.id].into_iter().collect());
        assert!(cached_whole(&f, 7, 1000).is_none() && cached_whole(&g, 0, 100).is_some());
        assert_eq!(missing(&f, &[(0, 10)]), PAGE);
    }
}
