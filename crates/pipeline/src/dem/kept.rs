//! A dataset's files kept whole (`[data] store = true`, docs/inputs.md §5.2 "Fetched data"): each
//! downloaded once into `sources/fetched/<dataset>/<file name>` as a compressed GeoTIFF (read back
//! and compared before it takes its name), a file the source doesn't have remembered as
//! `<file name>.none`. A store this worker can't write, or only reads (a task's worker), gives what
//! it has whole, and the rest is read in place (nothing written or removed there).

use crate::geotiff::{key, write_f32, Tiff};
use anyhow::{ensure, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store::range::{PlainFile, RangeRead};

/// A file from the store: its stored copy, or the file read in place.
pub enum Kept {
    Stored(PathBuf),
    InPlace(Arc<dyn RangeRead>),
}

/// Held while a file is read from its source (opened and read whole): one at a time in a process,
/// a zip's members coming from one server, asked politely. Held around the network alone, never
/// across the decoding and writing, which run rayon jobs: a thread waiting on a job of its own
/// runs other jobs meanwhile, and one of those may come here again, so the lock must not be held
/// then (it would be asked for again on the thread that holds it).
static DOWNLOADING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Whether `e` says the store can't be written here.
fn unwritable(e: &std::io::Error) -> bool {
    matches!(e.kind(), std::io::ErrorKind::ReadOnlyFilesystem | std::io::ErrorKind::PermissionDenied)
}

/// File `name` from the store `dir`, copied there from `remote` the first time; None for a file the
/// source doesn't have, remembered as `<name>.none`. A stored copy that isn't whole, or (`again`)
/// won't read, is taken again.
pub fn kept(dir: &Path, name: &str, remote: &dyn Fn() -> Result<Option<Arc<dyn RangeRead>>>, again: bool, read_only: bool) -> Result<Option<Kept>> {
    let f = dir.join(name);
    let in_place = || -> Result<Option<Kept>> { Ok(remote()?.map(Kept::InPlace)) };
    if f.exists() {
        if !again && crate::whole::tiff_file_whole(&f) {
            return Ok(Some(Kept::Stored(f)));
        }
        if read_only {
            return in_place();
        }
        match std::fs::remove_file(&f) {
            Err(e) if unwritable(&e) => return in_place(),
            r => r?,
        }
    } else if dir.join(format!("{name}.none")).exists() {
        return Ok(None);
    }
    if read_only {
        return in_place();
    }
    match std::fs::create_dir_all(dir) {
        Err(e) if unwritable(&e) => return in_place(),
        r => r?,
    }
    let host = store::sys::hostname().unwrap_or_else(|| "unknown".into());
    let tmp = dir.join(format!("{name}.{host}.{}.tmp", store::sys::pid()));
    // The source, opened and read whole, one file at a time (`DOWNLOADING`); the lock let go before
    // the bytes are decoded and written.
    let (bytes, mut w) = {
        let _one = DOWNLOADING.lock().unwrap_or_else(|e| e.into_inner());
        let Some(src) = remote()? else {
            match std::fs::write(dir.join(format!("{name}.none")), b"") {
                Err(e) if unwritable(&e) => {}
                r => r?,
            }
            return Ok(None);
        };
        // (The temporary file first: a store this worker can't write is known before the file is
        // read.)
        let w = match std::fs::File::create(&tmp) {
            Err(e) if unwritable(&e) => return Ok(Some(Kept::InPlace(src))),
            r => r?,
        };
        let r = (|| -> Result<Vec<u8>> {
            let len = src.len()?;
            let mut bytes = Vec::with_capacity(len as usize);
            while (bytes.len() as u64) < len {
                let n = (len - bytes.len() as u64).min(8 << 20) as usize;
                bytes.extend(src.read_at(bytes.len() as u64, n)?);
            }
            Ok(bytes)
        })();
        match r {
            Ok(b) => (b, w),
            Err(e) => {
                drop(w);
                std::fs::remove_file(&tmp).ok();
                return Err(e);
            }
        }
    };
    let r = (|| -> Result<()> {
        let t = Tiff::open(Arc::new(bytes)).with_context(|| name.to_string())?;
        let img = t.level(0)?.clone();
        let data = t.read_window(0, 0, 0, img.width, img.height)?;
        let gt = t.transform(0)?;
        let epsg = t.geo_keys().short(key::GEOGRAPHIC_TYPE).unwrap_or(4326);
        let out = write_f32(img.width, img.height, &data, 512, gt, epsg, t.nodata())?;
        {
            use std::io::Write;
            w.write_all(&out)?;
            w.sync_all()?;
            drop(w);
        }
        let back = Tiff::open(Arc::new(PlainFile::open(&tmp)?))?;
        let got = back.read_window(0, 0, 0, img.width, img.height)?;
        let eq = |a: f64, b: f64| a == b || (a.is_nan() && b.is_nan());
        let same = got.len() == data.len() && got.iter().zip(&data).all(|(&a, &b)| eq(a as f64, b as f64));
        let nodata_same = match (back.nodata(), t.nodata()) {
            (Some(a), Some(b)) => eq(a, b),
            (a, b) => a.is_none() && b.is_none(),
        };
        ensure!(same && nodata_same && back.transform(0)? == gt, "{name}: the copy written to the store reads back different");
        std::fs::rename(&tmp, &f)?;
        Ok(())
    })();
    if r.is_err() {
        std::fs::remove_file(&tmp).ok();
    }
    r?;
    Ok(Some(Kept::Stored(f)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Copies made inside rayon jobs, several files at once (as elev's distance scan opens files
    /// from its pool's threads): the decoding and writing run rayon jobs of their own, and a thread
    /// waiting on one runs other jobs meanwhile, among them another file's copy. With the source
    /// lock held across that, the thread would ask for the lock it holds; it's held around the
    /// read alone, so every copy finishes.
    #[test]
    fn copies_made_inside_rayon_jobs_dont_hang() {
        use rayon::prelude::*;
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("fabdem");
        let (w, h) = (1536u32, 1536u32);
        let files: Vec<(String, Arc<Vec<u8>>)> = (0..8)
            .map(|k| {
                let data: Vec<f32> = (0..w * h).map(|i| (i % 977) as f32 + k as f32).collect();
                (format!("f{k}.tif"), Arc::new(write_f32(w, h, &data, 512, [k as f64, 0.001, 0.0, 10.0, 0.0, -0.001], 4326, Some(-9999.0)).unwrap()))
            })
            .collect();
        let (tx, rx) = std::sync::mpsc::channel();
        let at = dir.clone();
        std::thread::spawn(move || {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(3).build().unwrap();
            let r: Result<Vec<Option<Kept>>> = pool.install(|| {
                files
                    .par_iter()
                    .map(|(name, bytes)| {
                        let b = bytes.clone();
                        let remote = move || -> Result<Option<Arc<dyn RangeRead>>> { Ok(Some(Arc::new(b.as_ref().clone()))) };
                        kept(&at, name, &remote, false, false)
                    })
                    .collect()
            });
            tx.send(r.map(|v| v.len())).ok();
        });
        match rx.recv_timeout(std::time::Duration::from_secs(120)) {
            Ok(r) => assert_eq!(r.unwrap(), 8),
            Err(_) => panic!("the copies didn't finish in two minutes: a copy waits on the lock it holds"),
        }
        for k in 0..8 {
            assert!(crate::whole::tiff_file_whole(&dir.join(format!("f{k}.tif"))), "f{k}");
        }
    }
}
