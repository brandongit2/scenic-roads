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
    let Some(src) = remote()? else {
        match std::fs::write(dir.join(format!("{name}.none")), b"") {
            Err(e) if unwritable(&e) => {}
            r => r?,
        }
        return Ok(None);
    };
    let host = store::sys::hostname().unwrap_or_else(|| "unknown".into());
    let tmp = dir.join(format!("{name}.{host}.{}.tmp", store::sys::pid()));
    // (The temporary file first: a store this worker can't write is known before the file is read.)
    let mut w = match std::fs::File::create(&tmp) {
        Err(e) if unwritable(&e) => return Ok(Some(Kept::InPlace(src))),
        r => r?,
    };
    let r = (|| -> Result<()> {
        let len = src.len()?;
        let mut bytes = Vec::with_capacity(len as usize);
        while (bytes.len() as u64) < len {
            let n = (len - bytes.len() as u64).min(8 << 20) as usize;
            bytes.extend(src.read_at(bytes.len() as u64, n)?);
        }
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
