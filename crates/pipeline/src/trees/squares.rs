//! The squares a z3 tile's blocks read, made ready first (the hub's work):
//! - **Canopy squares** read from this Mac's whole copies in the cache (`--chm`, the units'
//!   copies) where it has them, else from the NAS's store (`--chm-store`): only the rows the run
//!   needs, read first in big reads (`rows_here`; whole squares were copied before task #606, ~2 GB a
//!   pair in Siberia, at 3–10 MB/s while the NAS was busy). A square not in the store is downloaded
//!   from Meta into it first, once: under a `<file>.lock` there, made exclusively, a job on the other
//!   Mac waiting for it (a lock not touched for 30 minutes is a holder that died). An empty file is
//!   Meta's "none there".
//! - **Leaf-type squares** made whole where they aren't (whole and tagged complete, or NALCMS's):
//!   by crate::leaftype, which fetches the EEA's chunks or resamples NALCMS, and says how far it
//!   is; with none to make, nothing is asked.

use super::{chm_name, leaf_name, LeafSources};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store::range::{PlainFile, RangeRead};

/// Whether `f` is kept whole: there, and empty (Meta's "none") or a whole TIFF. One that isn't is
/// deleted, to be taken again.
fn kept_whole(f: &Path) -> bool {
    let Ok(m) = std::fs::metadata(f) else { return false };
    if m.len() == 0 || crate::whole::tiff_file_whole(f) {
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
            && (std::fs::metadata(&copy).is_ok_and(|m| m.len() == 0) || crate::whole::tiff_file_whole(&copy))
        {
            Here::Local(copy)
        } else {
            store::cachefile::release(&copy);
            let kept = store.join(&name);
            fetch_once(store, &kept, urls.get(&name).map(String::as_str), &|f| said((j as f64 + f) / 2.0))?;
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

/// A canopy file at `path` (on the NAS) for a run whose blocks read its rows `rows` (first and
/// last, of its full-resolution image): the bytes of the strips holding them (a canopy square's
/// strips are rows of its whole width) read first in big reads, `ROWS_READERS` at once, into
/// `local` (the run's scratch), and read from there; anything else (its header and strip arrays)
/// where it lies. The same bytes as the file's, so the same tiles. `done` is told the bytes read so
/// far, of how many.
pub fn rows_here(
    path: &Path,
    rows: (u32, u32),
    local: &Path,
    done: &(dyn Fn(u64, u64) + Sync),
) -> Result<Arc<dyn RangeRead>> {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    use store::sys::PosIo;
    let what = || path.display().to_string();
    let src: Arc<dyn RangeRead> = Arc::new(PlainFile::open(path).with_context(what)?);
    let (lo, hi) = {
        let t = crate::geotiff::Tiff::open(src.clone()).with_context(what)?;
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
        return Ok(src);
    }
    let f = std::fs::File::create(local).with_context(|| local.display().to_string())?;
    let len = hi - lo;
    let (next, got) = (AtomicU64::new(0), AtomicU64::new(0));
    std::thread::scope(|s| -> Result<()> {
        let hs: Vec<_> = (0..ROWS_READERS)
            .map(|_| {
                s.spawn(|| -> Result<()> {
                    loop {
                        let at = next.fetch_add(ROWS_READ, Relaxed);
                        if at >= len {
                            return Ok(());
                        }
                        let n = ROWS_READ.min(len - at);
                        let b = src.read_at(lo + at, n as usize).with_context(what)?;
                        f.write_all_at(&b, at).with_context(|| local.display().to_string())?;
                        done(got.fetch_add(n, Relaxed) + n, len);
                    }
                })
            })
            .collect();
        for h in hs {
            h.join().map_err(|_| anyhow::anyhow!("a reader of {} panicked", what()))??;
        }
        Ok(())
    })?;
    drop(f);
    let local = PlainFile::open(local).with_context(|| local.display().to_string())?;
    Ok(Arc::new(Rows { src, local, at: lo, len }))
}

/// A file read where it lies but for a span of it read before into a local file (`rows_here`).
struct Rows {
    src: Arc<dyn RangeRead>,
    local: PlainFile,
    at: u64,
    len: u64,
}

impl RangeRead for Rows {
    fn len(&self) -> Result<u64, store::IoError> {
        self.src.len()
    }

    fn read_at(&self, off: u64, len: usize) -> Result<Vec<u8>, store::IoError> {
        if off >= self.at && off.saturating_add(len as u64) <= self.at + self.len {
            self.local.read_at(off - self.at, len)
        } else {
            self.src.read_at(off, len)
        }
    }
}

/// The NAS's copy `kept`, whole; else the right to download it there (`<file>.lock`), or the copy
/// the holder downloads, waited for. `coming` is told how much of this job's download has come.
fn fetch_once(store: &Path, kept: &Path, url: Option<&str>, coming: &dyn Fn(f64)) -> Result<()> {
    std::fs::create_dir_all(store).with_context(|| store.display().to_string())?;
    let lock = kept.with_file_name(format!("{}.lock", kept.file_name().unwrap_or_default().to_string_lossy()));
    let mut waiting = false;
    while !kept_whole(kept) {
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&lock) {
            Ok(mut f) => {
                use std::io::Write;
                f.write_all(format!("{} {}", store::sys::host(), std::process::id()).as_bytes()).ok();
                drop(f);
                let r = download(url, kept, coming);
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

/// The file at `url` (the canopy source's index's: `super::canopy_urls`) into `path` (by a
/// temporary name, flushed), whole: a body shorter than its Content-Length, or not a whole TIFF, is
/// tried again. An empty file when the index lists none (`url` None), or Meta has none (404, or
/// S3's 403 for a key that isn't there), so it says twice, a moment apart. Anything else (writing
/// the file on the NAS too) is tried six times, then fails. `coming` is told how much has come
/// (0–1), at most once a second.
#[cfg(not(target_os = "wasi"))]
fn download(url: Option<&str>, path: &Path, coming: &dyn Fn(f64)) -> Result<()> {
    use std::io::{Read, Write};
    let Some(url) = url else {
        std::fs::write(path, b"").with_context(|| path.display().to_string())?;
        return Ok(());
    };
    // (A square is up to 2 GB: two hours for it, so it comes at 0.3 MB/s too.)
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(7200)))
        .timeout_connect(Some(std::time::Duration::from_secs(60)))
        .user_agent(crate::fetch::USER_AGENT)
        .http_status_as_error(false)
        .build()
        .into();
    let tmp = store::whole::tmp_name(path);
    let mut missing = 0;
    let mut last = String::new();
    for attempt in 0..6u32 {
        let got = (|| -> Result<Option<String>> {
            crate::fetch::online(url)?;
            let _w = store::waits::net();
            let found = match agent.get(url).call() {
                Ok(r) if matches!(r.status().as_u16(), 403 | 404) => None,
                Ok(r) => Some(r),
                Err(e) => return Ok(Some(e.to_string())),
            };
            let Some(mut r) = found else {
                missing += 1;
                if missing == 2 {
                    if let Err(e) = std::fs::write(path, b"") {
                        return Ok(Some(format!("{}: {e}", path.display())));
                    }
                    return Ok(None);
                }
                return Ok(Some("status 404".into()));
            };
            match r.status().as_u16() {
                200 => {}
                c => return Ok(Some(format!("status {c}"))),
            }
            let want: Option<u64> =
                r.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok());
            // (The NAS not taking it now: tried again, as a download cut short.)
            let on_nas = |e: std::io::Error| -> Result<Option<String>> { Ok(Some(format!("{}: {e}", tmp.display()))) };
            let mut f = match std::fs::File::create(&tmp) {
                Ok(f) => f,
                Err(e) => return on_nas(e),
            };
            let mut body = r.body_mut().with_config().limit(u64::MAX).reader();
            let mut b = vec![0u8; 16 << 20];
            let (mut n, mut at) = (0u64, std::time::Instant::now());
            loop {
                let k = match body.read(&mut b) {
                    Ok(0) => break,
                    Ok(k) => k,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => return Ok(Some(e.to_string())),
                };
                if let Err(e) = f.write_all(&b[..k]) {
                    return on_nas(e);
                }
                n += k as u64;
                if let Some(w) = want.filter(|&w| w > 0) {
                    if at.elapsed() >= std::time::Duration::from_secs(1) {
                        at = std::time::Instant::now();
                        coming(n as f64 / w as f64);
                    }
                }
            }
            if let Err(e) = f.sync_all() {
                return on_nas(e);
            }
            drop(f);
            if want.is_some_and(|w| w != n) {
                return Ok(Some(format!("{n} of {} bytes", want.unwrap_or(0))));
            }
            if !crate::whole::tiff_file_whole(&tmp) {
                return Ok(Some("not a whole TIFF".into()));
            }
            if let Err(e) = std::fs::rename(&tmp, path) {
                return on_nas(e);
            }
            Ok(None)
        })();
        match got {
            Ok(None) => return Ok(()),
            Ok(Some(why)) => last = why,
            Err(e) => {
                std::fs::remove_file(&tmp).ok();
                return Err(e);
            }
        }
        std::fs::remove_file(&tmp).ok();
        std::thread::sleep(std::time::Duration::from_secs(if missing > 0 { 5 } else { 1 << attempt }));
    }
    bail!("download failed: {url}: {last}")
}

#[cfg(target_os = "wasi")]
fn download(url: Option<&str>, path: &Path, _coming: &dyn Fn(f64)) -> Result<()> {
    match url {
        None => std::fs::write(path, b"").with_context(|| path.display().to_string()),
        Some(u) => bail!("{u}: not in the cache, and there's no network here"),
    }
}

/// Whether leaf-type square `p` was made whole (tagged so; NALCMS's always are), not only over some
/// regions, and is whole on the disk.
pub fn complete(p: &Path) -> bool {
    if !crate::whole::tiff_file_whole(p) {
        return false;
    }
    let Ok(t) = store::range::PlainFile::open(p)
        .map_err(anyhow::Error::from)
        .and_then(|f| crate::geotiff::Tiff::open(std::sync::Arc::new(f)))
    else {
        return false;
    };
    t.metadata_item("complete") == Some("1") || t.metadata_item("source").is_some_and(|s| s.starts_with("NALCMS"))
}

/// The leaf-type squares among `sqs` (top, left) that `dir` lacks whole, made in this process
/// (crate::leaftype, its NALCMS GeoTIFF kept in `dir`'s parent; `leaf` the declared sources), which
/// says how far it is: inside the European source's box from it, inside the North American's from
/// it, none elsewhere (no source). With none to make, nothing is asked. (WebAssembly has no
/// network: there a square to make is an error.)
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
    #[cfg(not(target_os = "wasi"))]
    return crate::leaftype::make(sqs, dir, leaf);
    #[cfg(target_os = "wasi")]
    bail!("{to_make} leaf-type squares to make, and there's no network here")
}
