//! A leaf-type square's GeoTIFF, byte for byte as GDAL 3.12.4 (libtiff, zlib) wrote it from
//! rasterio (`driver="GTiff"`, one 8-bit band, `tiled=True` 512 by 512, `compress="deflate"`,
//! EPSG:4326 from a north-up origin, nodata 255, its tags):
//!
//! - the directory GDAL wrote when the file was made, at byte 8: no metadata yet, the tiles'
//!   offsets and sizes zero, then the georeferencing's values;
//! - the tiles in order, each zlib's at level 6 (zlib 1.3's own deflate: zlib-rs's differs),
//!   the parts past the raster's edge 0;
//! - the directory as GDAL rewrote it on closing, at the next even byte: the same tags with the
//!   metadata (its XML, the items by name) and the tiles' sizes and offsets, the header pointing
//!   at it.

use anyhow::{ensure, Result};

const TILE: usize = 512;

/// zlib's `compress2` at level 6.
fn deflate(raw: &[u8]) -> Result<Vec<u8>> {
    let mut out = vec![0u8; raw.len() + raw.len() / 1000 + 64];
    let mut n = out.len() as libz_sys::uLongf;
    // SAFETY: both buffers are live for the call, their lengths given.
    let r = unsafe { libz_sys::compress2(out.as_mut_ptr(), &mut n, raw.as_ptr(), raw.len() as libz_sys::uLong, 6) };
    ensure!(r == libz_sys::Z_OK, "zlib: {r}");
    out.truncate(n as usize);
    Ok(out)
}

/// One directory entry: tag, type, count, value bytes.
type Entry = (u16, u16, u32, Vec<u8>);

fn shorts(v: &[u16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn doubles(v: &[f64]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn longs(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// `entries` written at `at` (even), their values that don't fit after it in the order libtiff
/// writes them (`order`: tags), each at an even byte.
fn directory(entries: &[Entry], order: &[u16], at: usize) -> Vec<u8> {
    let size = 2 + 12 * entries.len() + 4;
    let mut place = std::collections::HashMap::new();
    let mut data = Vec::new();
    for tag in order {
        let e = entries.iter().find(|e| e.0 == *tag).expect("an entry for each placed tag");
        if e.3.len() > 4 {
            place.insert(*tag, at + size + data.len());
            data.extend_from_slice(&e.3);
            if data.len() % 2 == 1 && *tag != *order.last().unwrap() {
                data.push(0);
            }
        }
    }
    let mut out = Vec::with_capacity(size + data.len());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (tag, typ, count, v) in entries {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&typ.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        if v.len() <= 4 {
            let mut b = [0u8; 4];
            b[..v.len()].copy_from_slice(v);
            out.extend_from_slice(&b);
        } else {
            out.extend_from_slice(&(place[tag] as u32).to_le_bytes());
        }
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&data);
    out
}

/// GDAL's metadata XML for `items` (name, value; sorted by name), with its closing NUL.
fn metadata(items: &[(&str, &str)]) -> Vec<u8> {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    let mut items = items.to_vec();
    items.sort();
    let mut x = String::from("<GDALMetadata>\n");
    for (k, v) in items {
        x.push_str(&format!("  <Item name=\"{}\">{}</Item>\n", esc(k), esc(v)));
    }
    x.push_str("</GDALMetadata>\n\0");
    x.into_bytes()
}

/// The square `a` (`n` by `n`, row by row) at `left`, `top` (degrees), `res` degrees a pixel,
/// nodata 255, its metadata `items`.
pub fn square(a: &[u8], n: usize, left: f64, top: f64, res: f64, items: &[(&str, &str)]) -> Result<Vec<u8>> {
    use rayon::prelude::*;
    ensure!(a.len() == n * n && n > 0 && n <= u16::MAX as usize, "a {n} by {n} square of {} values", a.len());
    let across = n.div_ceil(TILE);
    let tiles: Vec<Vec<u8>> = (0..across * across)
        .into_par_iter()
        .map(|t| {
            let (tx, ty) = (t % across, t / across);
            let mut b = vec![0u8; TILE * TILE];
            for r in 0..TILE {
                let y = ty * TILE + r;
                if y >= n {
                    break;
                }
                let x0 = tx * TILE;
                let w = TILE.min(n - x0);
                b[r * TILE..r * TILE + w].copy_from_slice(&a[y * n + x0..y * n + x0 + w]);
            }
            deflate(&b)
        })
        .collect::<Result<_>>()?;
    let k = tiles.len();
    let geokeys: [u16; 32] = [
        1, 1, 0, 7, 1024, 0, 1, 2, 1025, 0, 1, 1, 2048, 0, 1, 4326, 2049, 34737, 7, 0, 2054, 0, 1, 9102, 2057, 34736,
        1, 1, 2059, 34736, 1, 0,
    ];
    let entries = |offsets: &[u32], counts: &[u32], meta: Option<&[u8]>| -> Vec<Entry> {
        let mut e: Vec<Entry> = vec![
            (256, 3, 1, shorts(&[n as u16])),
            (257, 3, 1, shorts(&[n as u16])),
            (258, 3, 1, shorts(&[8])),
            (259, 3, 1, shorts(&[8])),
            (262, 3, 1, shorts(&[1])),
            (277, 3, 1, shorts(&[1])),
            (284, 3, 1, shorts(&[1])),
            (317, 3, 1, shorts(&[1])),
            (322, 3, 1, shorts(&[TILE as u16])),
            (323, 3, 1, shorts(&[TILE as u16])),
            (324, 4, k as u32, longs(offsets)),
            (325, 4, k as u32, longs(counts)),
            (339, 3, 1, shorts(&[1])),
            (33550, 12, 3, doubles(&[res, res, 0.0])),
            (33922, 12, 6, doubles(&[0.0, 0.0, 0.0, left, top, 0.0])),
            (34735, 3, 32, shorts(&geokeys)),
            (34736, 12, 2, doubles(&[298.257223563, 6378137.0])),
            (34737, 2, 8, b"WGS 84|\0".to_vec()),
        ];
        if let Some(m) = meta {
            e.push((42112, 2, m.len() as u32, m.to_vec()));
        }
        e.push((42113, 2, 4, b"255\0".to_vec()));
        e
    };
    let order = [325, 324, 33550, 33922, 34735, 34736, 34737, 42112];
    let zeros = vec![0u32; k];
    let mut out = b"II*\0\0\0\0\0".to_vec();
    out.extend(directory(&entries(&zeros, &zeros, None), &order[..7], 8));
    let mut offsets = Vec::with_capacity(k);
    for t in &tiles {
        offsets.push(u32::try_from(out.len())?);
        out.extend_from_slice(t);
    }
    let counts: Vec<u32> = tiles.iter().map(|t| t.len() as u32).collect();
    if out.len() % 2 == 1 {
        out.push(0);
    }
    let at = out.len();
    let meta = metadata(items);
    out.extend(directory(&entries(&offsets, &counts, Some(&meta)), &order, at));
    out[4..8].copy_from_slice(&u32::try_from(at)?.to_le_bytes());
    Ok(out)
}
