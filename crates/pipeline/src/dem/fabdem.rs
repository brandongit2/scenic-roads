//! FABDEM v1-2 (30 m: Copernicus DEM with forests and buildings removed; University of Bristol,
//! CC BY-NC-SA 4.0): 1° tiles, inside Bristol's 10° zips (stored uncompressed). With a store
//! ($SCENIC_FABDEM_STORE, the NAS's `sources/fabdem/`) each tile is downloaded once and kept there
//! as a compressed GeoTIFF (read back and compared before it takes its name), and a tile the zip
//! doesn't have (open sea) is remembered as `<tile>.none`. Without one, tiles are read in place
//! inside the zips, by range; so are those a store this worker can't write (the NAS's read where it
//! lies, by a browser's task) doesn't have yet.

use crate::fetch::{zip_member, Fetch};
use crate::geotiff::{key, write_f32, Tiff};
use anyhow::{ensure, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store::range::{PlainFile, RangeRead};

pub const BASE: &str = "https://data.bris.ac.uk/datasets/s5hqmjcdj8yo2ibzi9b4ew3sn";

/// A tile's or a zip corner's name, by its south-west corner: N43E007.
pub fn name(lat0: i64, lon0: i64) -> String {
    format!("{}{:02}{}{:03}", if lat0 >= 0 { 'N' } else { 'S' }, lat0.abs(), if lon0 >= 0 { 'E' } else { 'W' }, lon0.abs())
}

/// The points `idx` grouped by 1° tile, in the tiles' order: ((tile, zip), points). Tiles are named
/// by their south-west corner, the 10° zips by their south-west and north-east corners.
pub fn groups(lon: &[f64], lat: &[f64], idx: &[u32]) -> Vec<((String, String), Vec<u32>)> {
    let mut keyed: Vec<(i64, u32)> = idx.iter().map(|&i| (((lat[i as usize].floor() as i64) + 90) * 1000 + (lon[i as usize].floor() as i64 + 180), i)).collect();
    keyed.sort_by_key(|k| k.0);
    let mut out: Vec<((String, String), Vec<u32>)> = Vec::new();
    let mut last = None;
    for (k, i) in keyed {
        if last != Some(k) {
            let (lat0, lon0) = (k.div_euclid(1000) - 90, k.rem_euclid(1000) - 180);
            let (la10, lo10) = (lat0.div_euclid(10) * 10, lon0.div_euclid(10) * 10);
            out.push(((name(lat0, lon0), format!("{}-{}", name(la10, lo10), name(la10 + 10, lo10 + 10))), Vec::new()));
            last = Some(k);
        }
        out.last_mut().unwrap().1.push(i);
    }
    out
}

pub fn zip_url(zname: &str) -> String {
    format!("{BASE}/{zname}_FABDEM_V1-2.zip")
}

pub fn member(tname: &str) -> String {
    format!("{tname}_FABDEM_V1-2.tif")
}

/// Tile `tname` read in place inside zip `zname` at Bristol; None when there's no such tile (the
/// zip isn't there, or doesn't list it: open sea).
pub fn remote(fetch: &dyn Fetch, zname: &str, tname: &str) -> Result<Option<Arc<dyn RangeRead>>> {
    let url = zip_url(zname);
    let Some(z) = fetch.open(&url)? else { return Ok(None) };
    zip_member(z, &member(tname)).with_context(|| format!("{url}: its file list can't be read; the unit is tried again later"))
}

/// A tile from the store: its stored copy, or (a store this worker can't write) the tile read in
/// place inside Bristol's zip.
pub enum Tile {
    Stored(PathBuf),
    InPlace(Arc<dyn RangeRead>),
}

/// Whether `e` says the store can't be written here.
fn unwritable(e: &std::io::Error) -> bool {
    matches!(e.kind(), std::io::ErrorKind::ReadOnlyFilesystem | std::io::ErrorKind::PermissionDenied)
}

/// Tile `tname` from the store, copied there from Bristol's zip `zname` the first time; None for a
/// tile the zip doesn't have, remembered as `<tile>.none`. A stored copy that isn't whole, or
/// (`again`) won't read, is taken again. A store this worker can't write, or only reads
/// (`read_only`: a task's worker), gives what it has whole; for the rest, the tile read in place
/// (and nothing written or removed there).
pub fn stored(fetch: &dyn Fetch, store: &Path, zname: &str, tname: &str, again: bool, read_only: bool) -> Result<Option<Tile>> {
    let f = store.join(member(tname));
    let in_place = || -> Result<Option<Tile>> {
        println!("FABDEM {tname}: the store {}: read in place", if read_only { "is only read here" } else { "can't be written here" });
        Ok(remote(fetch, zname, tname)?.map(Tile::InPlace))
    };
    if f.exists() {
        if !again && crate::whole::tiff_file_whole(&f) {
            return Ok(Some(Tile::Stored(f)));
        }
        if read_only {
            return in_place();
        }
        println!("FABDEM {tname}: the stored copy {}: taken again", if again { "won’t read" } else { "isn’t whole" });
        match std::fs::remove_file(&f) {
            Err(e) if unwritable(&e) => return in_place(),
            r => r?,
        }
    } else if store.join(format!("{tname}.none")).exists() {
        return Ok(None);
    }
    if read_only {
        return in_place();
    }
    match std::fs::create_dir_all(store) {
        Err(e) if unwritable(&e) => return in_place(),
        r => r?,
    }
    let Some(src) = remote(fetch, zname, tname)? else {
        match std::fs::write(store.join(format!("{tname}.none")), b"") {
            Err(e) if unwritable(&e) => {}
            r => r?,
        }
        return Ok(None);
    };
    let host = store::sys::hostname().unwrap_or_else(|| "unknown".into());
    let tmp = store.join(format!("{}.{host}.{}.tmp", member(tname), store::sys::pid()));
    // (The temporary file first: a store this worker can't write is known before the tile is read.)
    let mut w = match std::fs::File::create(&tmp) {
        Err(e) if unwritable(&e) => {
            println!("FABDEM {tname}: the store can't be written here: read in place");
            return Ok(Some(Tile::InPlace(src)));
        }
        r => r?,
    };
    let r = (|| -> Result<()> {
        // The member whole, in large reads.
        let len = src.len()?;
        let mut bytes = Vec::with_capacity(len as usize);
        while (bytes.len() as u64) < len {
            let n = (len - bytes.len() as u64).min(8 << 20) as usize;
            bytes.extend(src.read_at(bytes.len() as u64, n)?);
        }
        let t = Tiff::open(Arc::new(bytes)).with_context(|| format!("FABDEM {tname}"))?;
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
        ensure!(same && nodata_same && back.transform(0)? == gt, "FABDEM {tname}: the copy written to the store reads back different");
        std::fs::rename(&tmp, &f)?;
        Ok(())
    })();
    if r.is_err() {
        std::fs::remove_file(&tmp).ok();
    }
    r?;
    Ok(Some(Tile::Stored(f)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_groups() {
        assert_eq!(name(43, 7), "N43E007");
        assert_eq!(name(-1, -70), "S01W070");
        let lon = [2.35, 2.9, -0.5, 7.1];
        let lat = [48.85, 48.1, 51.5, 43.7];
        let g = groups(&lon, &lat, &[0, 1, 2, 3]);
        let names: Vec<(&str, &str, Vec<u32>)> = g.iter().map(|((t, z), i)| (t.as_str(), z.as_str(), i.clone())).collect();
        assert_eq!(names, vec![("N43E007", "N40E000-N50E010", vec![3]), ("N48E002", "N40E000-N50E010", vec![0, 1]), ("N51W001", "N50W010-N60E000", vec![2])]);
    }
}
