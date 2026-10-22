//! The squares a z3 tile's blocks read, made ready first (the hub's work):
//! - **Canopy squares** copied into the cache (`--chm`, the units' too) from the NAS's store
//!   (`--chm-store`), downloaded from Meta into the store first, once: under a `<file>.lock` there,
//!   made exclusively, a job on the other Mac waiting for it (a lock not touched for 30 minutes is a
//!   holder that died). An empty file is Meta's "none there". A copy that isn't whole is taken
//!   again (crate::whole).
//! - **Leaf-type squares** made whole where they aren't (whole and tagged complete, or NALCMS's):
//!   by dem/leaftype.py (`--make`), which fetches the EEA's chunks or reprojects NALCMS, and says
//!   how far it is; with none to make, nothing is run.

use super::{chm_name, leaf_name, LeafSources};
use anyhow::{bail, Context, Result};
use std::path::Path;

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

/// Canopy square (`top`, `left`)'s cover and height files in `chm`: copied from the NAS's `store`,
/// or downloaded into it first (once); false when Meta has none there. `said` is told how much of
/// the square is done (0–1): each file half of it, a download's share as it comes.
pub fn canopy(
    chm: &Path,
    store: &Path,
    top: i32,
    left: i32,
    said: &dyn Fn(f64),
    urls: &std::collections::BTreeMap<String, String>,
) -> Result<bool> {
    let mut there = true;
    for (j, kind) in ["cover5m", "p95"].into_iter().enumerate() {
        let name = chm_name(top, left, kind);
        let p = chm.join(&name);
        // Held for the rest of the job (store::cachefile: the blocks read it by name, and a square
        // room-making deleted would fail the run, or read as none there), and marked used; taken
        // again when it isn't here whole.
        let mut held = false;
        for _ in 0..3 {
            if store::cachefile::hold_existing(&p)?.is_some() {
                if std::fs::metadata(&p).is_ok_and(|m| m.len() == 0) || crate::whole::tiff_file_whole(&p) {
                    held = true;
                    break;
                }
                eprintln!("canopy: {} isn't whole: taken again", p.display());
                store::cachefile::discard(&p);
            }
            let kept = store.join(&name);
            fetch_once(store, &kept, urls.get(&name).map(String::as_str), &|f| said((j as f64 + f) / 2.0))?;
            store::cachefile::create(&p, &mut |t| {
                let (n, want) = (store::sys::copy_data(&kept, t)?, std::fs::metadata(&kept)?.len());
                if n != want {
                    return Err(std::io::Error::other(format!("{}: {n} of {want} bytes copied", kept.display())));
                }
                Ok(())
            })
            .with_context(|| p.display().to_string())?;
            said((j + 1) as f64 / 2.0);
        }
        anyhow::ensure!(held, "{}: not here whole after three copies", p.display());
        if std::fs::metadata(&p).with_context(|| p.display().to_string())?.len() == 0 {
            there = false;
        }
    }
    Ok(there)
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
                f.write_all(format!("{} {}", crate::agent::cond::host(), std::process::id()).as_bytes()).ok();
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
    let tmp = crate::whole::tmp_name(path);
    let mut missing = 0;
    let mut last = String::new();
    for attempt in 0..6u32 {
        let got = (|| -> Result<Option<String>> {
            crate::fetch::online(url)?;
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
/// regions, and is whole on the disk (leaftype.py's `complete`).
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

/// The leaf-type squares among `sqs` (top, left) that `dir` lacks whole, made by `dem`'s leaftype.py
/// (`--make`, its NALCMS GeoTIFF kept in `dir`'s parent; the run's sources file `file`, of which
/// `leaf` is the leaf-type sources), which says how far it is: inside the European source's box from
/// it, inside the North American's from it, none elsewhere (no source).
pub fn leaf_types(sqs: &[(i32, i32)], dir: &Path, dem: &Path, leaf: &LeafSources, file: &Path) -> Result<()> {
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
    let mut c = store::spawn::command("uv");
    c.current_dir(dem)
        .args(["run", "python", "leaftype.py", "--make"])
        .arg(dir)
        .arg("--sources")
        .arg(std::path::absolute(file)?)
        .args(sqs.iter().map(|(t, l)| format!("{t},{l}")));
    crate::timings::child(&mut c);
    let st = store::spawn::status(&mut c).context("run leaftype.py")?;
    anyhow::ensure!(st.success(), "leaftype.py: {st}");
    Ok(())
}
