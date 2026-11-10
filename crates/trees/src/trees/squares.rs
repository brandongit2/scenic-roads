//! The squares a z3 tile's blocks read, made ready first (the hub's work):
//! - **Canopy squares** read from this Mac's whole copies in the cache (`--chm`, the units'
//!   copies) where it has them, else from the NAS's store (`--chm-store`): only the rows the run
//!   needs, read first in big reads and kept in the cache for the runs after (`rows_here`,
//!   `RowsCache`; whole squares were copied before task #606, ~2 GB a pair in Siberia, at 3–10 MB/s
//!   while the NAS was busy). A square not in the store is downloaded
//!   from Meta into it first, once: under a `<file>.lock` there, made exclusively, a job on the other
//!   Mac waiting for it (a lock not touched for 30 minutes is a holder that died). An empty file is
//!   Meta's "none there".
//! - **Leaf-type squares** made whole where they aren't (whole and tagged complete; NALCMS's also
//!   resampled as today: their `source` tag):
//!   by crate::leaftype, which fetches the EEA's chunks or resamples NALCMS, and says how far it
//!   is; with none to make, nothing is asked.

use super::{chm_name, leaf_name, LeafSources};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store::range::{PlainFile, RangeRead};

/// Whether `f` is kept whole: there, and empty (Meta's "none") or a whole TIFF. One that isn't is
/// deleted, to be taken again.
fn kept_whole(f: &Path) -> bool {
    let Ok(m) = std::fs::metadata(f) else { return false };
    if m.len() == 0 || codecs::whole::tiff_file_whole(f) {
        return true;
    }
    eprintln!("canopy: {} isn't whole: taken again", f.display());
    std::fs::remove_file(f).ok();
    false
}

/// Canopy square (`top`, `left`)'s cover and height files where the run reads them: this Mac's
/// whole copies in `chm` when it has them (the z6 tiles' canopy step leaves them there), held for the
/// rest of the job (store::cachefile: room-making leaves them) and marked used; else the NAS's
/// `store`, downloaded into it first (once) when it isn't there whole, read where it lies (only the
/// rows the run needs: `rows_here`). None when Meta has none there. `said` is told how much of the
/// square is ready (0–1): each file half of it, a download's share as it comes.
pub fn canopy(
    chm: &Path,
    store: &Path,
    top: i32,
    left: i32,
    said: &dyn Fn(f64),
    urls: &std::collections::BTreeMap<String, String>,
) -> Result<Option<[Here; 2]>> {
    let mut out = Vec::new();
    for (j, kind) in ["cover5m", "p95"].into_iter().enumerate() {
        let name = chm_name(top, left, kind);
        let copy = chm.join(&name);
        let here = if store::cachefile::hold_existing(&copy)?.is_some()
            && (std::fs::metadata(&copy).is_ok_and(|m| m.len() == 0) || codecs::whole::tiff_file_whole(&copy))
        {
            Here::Local(copy)
        } else {
            store::cachefile::release(&copy);
            let kept = store.join(&name);
            ensure_square(store, &kept, urls.get(&name).map(String::as_str), &|f| said((j as f64 + f) / 2.0))?;
            Here::Nas(kept)
        };
        said((j + 1) as f64 / 2.0);
        let p = here.path();
        if std::fs::metadata(p).with_context(|| p.display().to_string())?.len() == 0 {
            return Ok(None);
        }
        out.push(here);
    }
    let h = out.pop().context("a height file")?;
    let c = out.pop().context("a cover file")?;
    Ok(Some([c, h]))
}

/// Where a run reads a canopy file (`canopy`).
#[derive(Clone, Debug, PartialEq)]
pub enum Here {
    /// This Mac's whole copy (`chm`), held.
    Local(PathBuf),
    /// The NAS's (`store`): only the rows the run needs, read first (`rows_here`).
    Nas(PathBuf),
}

impl Here {
    pub fn path(&self) -> &Path {
        match self {
            Here::Local(p) | Here::Nas(p) => p,
        }
    }
}

/// The size of each read `rows_here` makes of the NAS (over SMB each read is a round trip, 50–120
/// ms under load: big ones), and how many it has under way at once.
const ROWS_READ: u64 = 8 << 20;
const ROWS_READERS: usize = 4;

/// Where the canopy rows a run reads from the NAS are kept past it: this Mac's cache's `blobs/`, the
/// file's name, its length and the span it holds in the name
/// (`sources/canopy/<file>.<length>.<first>-<end>.rows`), so the next z6 tile's run reading the same
/// strips (a strip is a row of the square's whole width: the tiles of a row read the same ones)
/// reads them here. As the other copies there: made and held through store::cachefile, the least
/// recently used freed by room-making (agentcore::agent::room, without asking the NAS: a square's
/// file is written once), and made only with room for them past the owner's room target
/// (jobrt::room), else read into the run's scratch and gone with it.
#[derive(Clone, Debug)]
pub struct RowsCache {
    dir: PathBuf,
    target: u64,
}

impl RowsCache {
    /// The agent's cache (`SCENIC_CACHE`) and room target; None without a cache (a run by hand, a
    /// test): every row read into the run's scratch.
    pub fn from_env() -> Option<RowsCache> {
        let cache = std::env::var_os(buildstore::coverage::CACHE_ENV)?;
        Some(RowsCache::new(Path::new(&cache), jobrt::room::job_target()))
    }

    pub fn new(cache: &Path, target: u64) -> RowsCache {
        RowsCache { dir: cache.join("blobs/sources/canopy"), target }
    }

    fn prefix(name: &str, file_len: u64) -> String {
        format!("{name}.{file_len}.")
    }

    /// The spans of a file kept here that meet `lo..hi`, each (first, end, its path), by first byte.
    fn spans(&self, name: &str, file_len: u64, lo: u64, hi: u64) -> Vec<(u64, u64, PathBuf)> {
        let pre = Self::prefix(name, file_len);
        let mut v: Vec<(u64, u64, PathBuf)> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let n = e.file_name();
                let (a, b) = n.to_str()?.strip_prefix(&pre)?.strip_suffix(".rows")?.split_once('-')?;
                let (a, b) = (a.parse().ok()?, b.parse().ok()?);
                (a < hi && lo < b).then(|| (a, b, e.path()))
            })
            .collect();
        v.sort();
        v
    }

    fn path_of(&self, name: &str, file_len: u64, lo: u64, hi: u64) -> PathBuf {
        self.dir.join(format!("{}{lo}-{hi}.rows", Self::prefix(name, file_len)))
    }
}

/// `lo..hi` less the spans `have` (sorted by first byte), in order.
fn gaps(lo: u64, hi: u64, have: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let (mut at, mut out) = (lo, Vec::new());
    for &(a, b) in have {
        if a > at {
            out.push((at, a.min(hi)));
        }
        at = at.max(b);
        if at >= hi {
            return out;
        }
    }
    if at < hi {
        out.push((at, hi));
    }
    out
}

/// The bytes `lo..hi` of `src` read into a new file `f`, in big reads, `ROWS_READERS` at once; `done`
/// told each read's bytes.
fn read_span(
    src: &dyn RangeRead,
    lo: u64,
    hi: u64,
    f: &std::fs::File,
    done: &(dyn Fn(u64) + Sync),
) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    use store::sys::PosIo;
    let len = hi - lo;
    let next = AtomicU64::new(0);
    std::thread::scope(|s| {
        let hs: Vec<_> = (0..ROWS_READERS)
            .map(|_| {
                s.spawn(|| -> std::io::Result<()> {
                    loop {
                        let at = next.fetch_add(ROWS_READ, Relaxed);
                        if at >= len {
                            return Ok(());
                        }
                        let n = ROWS_READ.min(len - at);
                        let b = src.read_at(lo + at, n as usize).map_err(std::io::Error::other)?;
                        f.write_all_at(&b, at)?;
                        done(n);
                    }
                })
            })
            .collect();
        hs.into_iter().try_for_each(|h| h.join().map_err(|_| std::io::Error::other("a canopy rows reader panicked"))?)
    })
}

/// A canopy file at `path` (on the NAS) for a run whose blocks read its rows `rows` (first and
/// last, of its full-resolution image): the bytes of the strips holding them (a canopy square's
/// strips are rows of its whole width) read here first: those `cache` has kept already where they
/// are, the others in big reads, `ROWS_READERS` at once, into `cache` where it has room for them,
/// else into `local` (the run's scratch; `.<n>` after it for each piece); anything else (its header
/// and strip arrays) where it lies. The same bytes as the file's, so the same tiles. `done` is told
/// the bytes read from the NAS so far, of how many. The file as the run reads it, and the bytes read
/// from the NAS.
pub fn rows_here(
    path: &Path,
    rows: (u32, u32),
    local: &Path,
    cache: Option<&RowsCache>,
    done: &(dyn Fn(u64, u64) + Sync),
) -> Result<(Arc<dyn RangeRead>, u64)> {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    let what = || path.display().to_string();
    let src: Arc<dyn RangeRead> = Arc::new(PlainFile::open(path).with_context(what)?);
    let (lo, hi) = {
        let t = buildstore::geotiff::Tiff::open(src.clone()).with_context(what)?;
        let img = t.level(0)?;
        let (across, bh) = (img.width.div_ceil(img.block_w.max(1)), img.block_h.max(1));
        let last = img.height.div_ceil(bh).saturating_sub(1);
        let (mut lo, mut hi) = (u64::MAX, 0u64);
        for by in (rows.0 / bh).min(last)..=(rows.1 / bh).min(last) {
            for bx in 0..across {
                if let Some((o, n)) = t.block_span(0, bx, by)? {
                    (lo, hi) = (lo.min(o), hi.max(o + n));
                }
            }
        }
        (lo, hi)
    };
    if lo >= hi {
        return Ok((src, 0));
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let file_len = src.len().with_context(what)?;
    // (Those kept here, held; one room-making took meanwhile is read again.)
    let mut pieces: Vec<(u64, u64, PlainFile)> = Vec::new();
    if let Some(c) = cache {
        for (a, b, p) in c.spans(&name, file_len, lo, hi) {
            if let Some(p) = store::cachefile::hold_existing(&p).ok().flatten() {
                pieces.push((a, b, PlainFile::open(&p).with_context(|| p.display().to_string())?));
            }
        }
    }
    let have: Vec<(u64, u64)> = pieces.iter().map(|p| (p.0, p.1)).collect();
    let todo = gaps(lo, hi, &have);
    let len: u64 = todo.iter().map(|g| g.1 - g.0).sum();
    let got = AtomicU64::new(0);
    let count = |n: u64| done(got.fetch_add(n, Relaxed) + n, len);
    for (k, &(a, b)) in todo.iter().enumerate() {
        let kept = cache
            .filter(|c| std::fs::create_dir_all(&c.dir).is_ok() && jobrt::room::free_past(&c.dir, c.target) >= b - a);
        let at = match kept {
            Some(c) => store::cachefile::hold(&c.path_of(&name, file_len, a, b), &mut |tmp| {
                read_span(&*src, a, b, &std::fs::File::create(tmp)?, &count)
            })
            .with_context(what)?,
            None => {
                let p = if k == 0 { local.to_path_buf() } else { PathBuf::from(format!("{}.{k}", local.display())) };
                let f = std::fs::File::create(&p).with_context(|| p.display().to_string())?;
                read_span(&*src, a, b, &f, &count).with_context(what)?;
                p
            }
        };
        pieces.push((a, b, PlainFile::open(&at).with_context(|| at.display().to_string())?));
    }
    Ok((Arc::new(Rows { src, pieces }), len))
}

/// A file read where it lies but for spans of it read before into local files (`rows_here`; they
/// may overlap: spans kept by runs that wanted other rows).
struct Rows {
    src: Arc<dyn RangeRead>,
    pieces: Vec<(u64, u64, PlainFile)>,
}

impl RangeRead for Rows {
    fn len(&self) -> Result<u64, store::IoError> {
        self.src.len()
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, store::IoError> {
        let end = off.saturating_add(len as u64);
        // (The piece holding `at` furthest on, then the next from where it ends.)
        let (mut at, mut out) = (off, Vec::new());
        while at < end {
            let Some(p) = self.pieces.iter().filter(|p| p.0 <= at && at < p.1).max_by_key(|p| p.1) else {
                // (What no piece holds, where it lies: the file's header and strip arrays.)
                return self.src.read_at(off, len);
            };
            let n = (p.1.min(end) - at) as usize;
            if n == len {
                return p.2.read_at(at - p.0, n);
            }
            out.extend(p.2.read_at(at - p.0, n)?);
            at += n as u64;
        }
        Ok(out)
    }
}

/// The NAS's copy `kept`, whole; else the right to download it there (`<file>.lock`), or the copy
/// the holder downloads, waited for. `coming` is told how much of this job's download has come.
pub fn ensure_square(store: &Path, kept: &Path, url: Option<&str>, coming: &dyn Fn(f64)) -> Result<()> {
    std::fs::create_dir_all(store).with_context(|| store.display().to_string())?;
    let lock = kept.with_file_name(format!("{}.lock", kept.file_name().unwrap_or_default().to_string_lossy()));
    let mut waiting = false;
    while !kept_whole(kept) {
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&lock) {
            Ok(mut f) => {
                use std::io::Write;
                f.write_all(format!("{} {}", store::sys::host(), std::process::id()).as_bytes()).ok();
                // (Touched each minute while the download lasts, its retries' waits too: a square
                // of 2 GB takes up to two hours, and a lock untouched for 30 minutes is a holder
                // that died. A crashed holder's is so taken over.)
                let done = std::sync::atomic::AtomicBool::new(false);
                let r = std::thread::scope(|s| {
                    s.spawn(|| {
                        let mut t = std::time::Instant::now();
                        while !done.load(std::sync::atomic::Ordering::Relaxed) {
                            std::thread::sleep(std::time::Duration::from_millis(200));
                            if t.elapsed() >= std::time::Duration::from_secs(60) {
                                t = std::time::Instant::now();
                                f.set_modified(std::time::SystemTime::now()).ok();
                            }
                        }
                    });
                    let r = download(url, kept, coming);
                    done.store(true, std::sync::atomic::Ordering::Relaxed);
                    r
                });
                drop(f);
                std::fs::remove_file(&lock).ok();
                r?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // (Gone meanwhile: tried again at once. Made by a Mac whose clock is ahead of this
                // one's, or not to be read now: new, waited for.)
                let age = match std::fs::metadata(&lock).and_then(|m| m.modified()) {
                    Ok(t) => t.elapsed().unwrap_or_default(),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(_) => std::time::Duration::ZERO,
                };
                if age > std::time::Duration::from_secs(1800) {
                    eprintln!("canopy: taking over {}", lock.display());
                    std::fs::remove_file(&lock).ok();
                    continue;
                }
                // (Said once, for the status: the square's progress stands while the holder's
                // download, which this job can't measure, goes on.)
                if !waiting {
                    eprintln!("canopy: waiting for another job's download ({})", lock.display());
                    waiting = true;
                }
                std::thread::sleep(std::time::Duration::from_secs(20));
            }
            Err(e) => return Err(e).with_context(|| format!("lock {}", lock.display())),
        }
    }
    Ok(())
}

/// The file at `url` (the canopy source's index's: `super::canopy_urls`) into `path`, whole
/// (buildstore::fetch::download: by a temporary name, flushed, a whole TIFF, then renamed). An empty
/// file when the index lists none (`url` None), or Meta has none (it says so twice). `coming` is
/// told how much has come (0–1).
fn download(url: Option<&str>, path: &Path, coming: &dyn Fn(f64)) -> Result<()> {
    let Some(url) = url else {
        std::fs::write(path, b"").with_context(|| path.display().to_string())?;
        return Ok(());
    };
    let tmp = store::whole::tmp_name(path);
    let got = buildstore::fetch::download(url, &tmp, &codecs::whole::tiff_file_whole, coming);
    let r = match got {
        Ok(true) => std::fs::rename(&tmp, path),
        Ok(false) => std::fs::write(path, b""),
        Err(e) => {
            std::fs::remove_file(&tmp).ok();
            return Err(e.context(format!("download failed: {url}")));
        }
    };
    std::fs::remove_file(&tmp).ok();
    r.with_context(|| path.display().to_string())
}

/// Whether leaf-type square `p` was made whole (tagged so), not only over some regions, and is whole
/// on the disk; a NALCMS square, also resampled as today (its `source`: crate::leaftype::NALCMS_SOURCE).
pub fn complete(p: &Path) -> bool {
    if !codecs::whole::tiff_file_whole(p) {
        return false;
    }
    let Ok(t) = store::range::PlainFile::open(p)
        .map_err(anyhow::Error::from)
        .and_then(|f| buildstore::geotiff::Tiff::open(std::sync::Arc::new(f)))
    else {
        return false;
    };
    t.metadata_item("complete") == Some("1")
        && t.metadata_item("source").is_none_or(|s| !s.starts_with("NALCMS") || s == crate::leaftype::NALCMS_SOURCE)
}

/// The leaf-type squares among `sqs` (top, left) that `dir` lacks whole, made in this process
/// (crate::leaftype, its NALCMS GeoTIFF kept in `dir`'s parent; `leaf` the declared sources), which
/// says how far it is: inside the European source's box from it, inside the North American's from
/// it, none elsewhere (no source). With none to make, nothing is asked.
pub fn leaf_types(sqs: &[(i32, i32)], dir: &Path, leaf: &LeafSources) -> Result<()> {
    let meets = |b: [f64; 4], top: i32, left: i32| {
        (left as f64) < b[2] && (left + 10) as f64 > b[0] && ((top - 10) as f64) < b[3] && top as f64 > b[1]
    };
    let to_make = sqs
        .iter()
        .filter(|&&(t, l)| {
            (meets(leaf.europe_box, t, l) || meets(leaf.north_america_box, t, l))
                && !complete(&dir.join(leaf_name(t, l)))
        })
        .count();
    if to_make == 0 {
        return Ok(());
    }
    crate::leaftype::make(sqs, dir, leaf)
}
