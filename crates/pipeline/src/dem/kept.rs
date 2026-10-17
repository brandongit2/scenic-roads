//! A dataset's files kept whole (`[data] store = true`, docs/inputs.md §5.2 "Fetched data"): each
//! in `sources/fetched/<dataset>/<file name>` as a compressed GeoTIFF, written here from a
//! downloaded copy (crate::dem::fabdem_fetch) and read back and compared before it takes its name.
//! Area builds only read the store (crate::dem::generic::stored).

use crate::geotiff::{key, write_f32, Tiff};
use anyhow::{ensure, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store::range::PlainFile;

/// Downloaded file `src` stored in `dir` as `name`: decoded, written as every stored file is
/// (`write_f32`, 512-pixel tiles), under a temporary name (`<name>.<host>.<pid>.tmp`) until it reads
/// back the same pixels, nodata and transform; then renamed into place.
pub fn store(dir: &Path, name: &str, src: &Path) -> Result<PathBuf> {
    let f = dir.join(name);
    std::fs::create_dir_all(dir)?;
    let t = Tiff::open(Arc::new(std::fs::read(src).with_context(|| src.display().to_string())?)).with_context(|| name.to_string())?;
    let img = t.level(0)?.clone();
    let data = t.read_window(0, 0, 0, img.width, img.height).with_context(|| name.to_string())?;
    let gt = t.transform(0)?;
    let epsg = t.geo_keys().short(key::GEOGRAPHIC_TYPE).unwrap_or(4326);
    let out = write_f32(img.width, img.height, &data, 512, gt, epsg, t.nodata())?;
    let host = store::sys::hostname().unwrap_or_else(|| "unknown".into());
    let tmp = dir.join(format!("{name}.{host}.{}.tmp", store::sys::pid()));
    let r = (|| -> Result<()> {
        {
            use std::io::Write;
            let mut w = std::fs::File::create(&tmp)?;
            w.write_all(&out)?;
            w.sync_all()?;
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
    Ok(f)
}
