//! GHSL's building heights (GHS-BUILT-H R2023A, ANBH) as JRC's 10° zips hold them, each a GeoTIFF
//! (crate::geotiff): the windows under a z6 tile that `bldprep` samples ([`windows`]), and the
//! check a downloaded zip passes before it's put in place ([`zip_bad`]: every member's CRC-32, as
//! Python's `zipfile` tests it, and a GeoTIFF among them).

use super::prep::{kind, Frame};
use anyhow::{bail, ensure, Context, Result};
use std::io::Read;
use std::path::Path;

/// A zip's member, as its central directory lists it.
struct Member {
    name: String,
    method: u16,
    crc: u32,
    csize: usize,
    size: usize,
    local: usize,
}

fn u16_at(b: &[u8], i: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(b.get(i..i + 2).context("cut short")?.try_into()?))
}

fn u32_at(b: &[u8], i: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(b.get(i..i + 4).context("cut short")?.try_into()?))
}

/// A zip's members (no Zip64: JRC's tiles are small).
fn members(b: &[u8]) -> Result<Vec<Member>> {
    let from = b.len().saturating_sub(22 + 0xffff);
    let end = (from..b.len().saturating_sub(21))
        .rev()
        .find(|&i| b[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
        .context("File is not a zip file")?;
    let n = u16_at(b, end + 10)? as usize;
    let mut at = u32_at(b, end + 16)? as usize;
    ensure!(at != 0xffff_ffff, "a Zip64 file");
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        ensure!(u32_at(b, at)? == 0x0201_4b50, "Bad magic number for central directory");
        let nl = u16_at(b, at + 28)? as usize;
        out.push(Member {
            method: u16_at(b, at + 10)?,
            crc: u32_at(b, at + 16)?,
            csize: u32_at(b, at + 20)? as usize,
            size: u32_at(b, at + 24)? as usize,
            local: u32_at(b, at + 42)? as usize,
            name: String::from_utf8_lossy(b.get(at + 46..at + 46 + nl).context("cut short")?).into_owned(),
        });
        at += 46 + nl + u16_at(b, at + 30)? as usize + u16_at(b, at + 32)? as usize;
    }
    Ok(out)
}

/// A member's bytes, inflated and checked against its CRC-32.
fn read_member(b: &[u8], m: &Member) -> Result<Vec<u8>> {
    ensure!(u32_at(b, m.local)? == 0x0403_4b50, "{}: Bad magic number for file header", m.name);
    let at = m.local + 30 + u16_at(b, m.local + 26)? as usize + u16_at(b, m.local + 28)? as usize;
    let raw = b.get(at..at + m.csize).with_context(|| format!("{}: cut short", m.name))?;
    let data = match m.method {
        0 => raw.to_vec(),
        8 => {
            let mut v = Vec::with_capacity(m.size);
            flate2::read::DeflateDecoder::new(raw)
                .read_to_end(&mut v)
                .with_context(|| format!("{}: inflate", m.name))?;
            v
        }
        x => bail!("{}: compression method {x}", m.name),
    };
    ensure!(data.len() == m.size && crc32fast::hash(&data) == m.crc, "{}: Bad CRC-32", m.name);
    Ok(data)
}

/// Why a downloaded tile's zip isn't whole (None: it is): a member failing its CRC-32, no GeoTIFF
/// in it, or not a zip at all.
pub fn zip_bad(path: &Path) -> Option<String> {
    let b = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return Some(format!("not a whole zip ({e})")),
    };
    let ms = match members(&b) {
        Ok(m) => m,
        Err(e) => return Some(format!("not a whole zip ({e:#})")),
    };
    for m in &ms {
        if read_member(&b, m).is_err() {
            return Some(format!("{} fails its CRC", m.name));
        }
    }
    if !ms.iter().any(|m| m.name.ends_with(".tif")) {
        return Some("no GeoTIFF in it".into());
    }
    None
}

/// The window under `b` (w, s, e, n) of each GHSL tile meeting it (`tiles`: name, size and box, by
/// name, from the index; their zips in `dir`), as frames: f32 rows, north first, a tile's nodata
/// and non-finite values 0. And the tiles' names.
pub fn windows(dir: &Path, tiles: &[(String, u64, [f64; 4])], b: [f64; 4]) -> Result<(Vec<Frame>, Vec<String>)> {
    let (mut frames, mut used) = (Vec::new(), Vec::new());
    for (name, _, bb) in tiles {
        if !(bb[0] <= b[2] && bb[2] >= b[0] && bb[1] <= b[3] && bb[3] >= b[1]) {
            continue;
        }
        let stem = name.strip_suffix(".zip").unwrap_or(name);
        let zip = std::fs::read(dir.join(name)).with_context(|| format!("GHSL {name}"))?;
        let tif_name = format!("{stem}.tif");
        let m = members(&zip)?
            .into_iter()
            .find(|m| m.name == tif_name)
            .with_context(|| format!("{name}: no {tif_name}"))?;
        let tif = crate::geotiff::Tiff::open(std::sync::Arc::new(read_member(&zip, &m)?))?;
        drop(zip);
        let gt = tif.transform(0)?;
        let img = tif.level(0)?;
        let (width, height) = (img.width as i64, img.height as i64);
        let c0 = ((b[0] - gt[0]) / gt[1]).floor().max(0.0) as i64;
        let c1 = width.min(((b[2] - gt[0]) / gt[1]).floor() as i64 + 1);
        let r0 = ((b[3] - gt[3]) / gt[5]).floor().max(0.0) as i64;
        let r1 = height.min(((b[1] - gt[3]) / gt[5]).floor() as i64 + 1);
        if c1 <= c0 || r1 <= r0 {
            continue;
        }
        let mut w = tif.read_window(0, c0 as u32, r0 as u32, (c1 - c0) as u32, (r1 - r0) as u32)?;
        let nodata = tif.nodata().filter(|v| v.is_finite()).map(|v| v as f32);
        for v in &mut w {
            if Some(*v) == nodata || !v.is_finite() {
                *v = 0.0;
            }
        }
        let header = serde_json::json!({
            "name": name, "transform": gt, "col_off": c0, "row_off": r0, "width": c1 - c0, "height": r1 - r0,
        });
        let data: Vec<u8> = w.iter().flat_map(|v| v.to_le_bytes()).collect();
        frames.push(Frame { kind: kind::GHSL, header, cols: [("data".to_string(), data)].into() });
        eprintln!("bldprep: GHSL {}: {} × {} pixels", &stem[stem.len().saturating_sub(8)..], c1 - c0, r1 - r0);
        used.push(name.clone());
    }
    Ok((frames, used))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A zip of (name, bytes) members, deflated.
    fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let (mut out, mut dir) = (Vec::new(), Vec::new());
        for (name, data) in files {
            let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(data).unwrap();
            let c = e.finish().unwrap();
            let local = out.len() as u32;
            let crc = crc32fast::hash(data);
            out.extend(0x0403_4b50u32.to_le_bytes());
            out.extend([20, 0, 0, 0, 8, 0, 0, 0, 0, 0]);
            out.extend(crc.to_le_bytes());
            out.extend((c.len() as u32).to_le_bytes());
            out.extend((data.len() as u32).to_le_bytes());
            out.extend((name.len() as u16).to_le_bytes());
            out.extend(0u16.to_le_bytes());
            out.extend(name.as_bytes());
            out.extend(&c);
            dir.extend(0x0201_4b50u32.to_le_bytes());
            dir.extend([20, 0, 20, 0, 0, 0, 8, 0, 0, 0, 0, 0]);
            dir.extend(crc.to_le_bytes());
            dir.extend((c.len() as u32).to_le_bytes());
            dir.extend((data.len() as u32).to_le_bytes());
            dir.extend((name.len() as u16).to_le_bytes());
            dir.extend([0u8; 12]);
            dir.extend(local.to_le_bytes());
            dir.extend(name.as_bytes());
        }
        let at = out.len() as u32;
        out.extend(&dir);
        out.extend(0x0605_4b50u32.to_le_bytes());
        out.extend([0u8; 4]);
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((dir.len() as u32).to_le_bytes());
        out.extend(at.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out
    }

    #[test]
    fn a_zip_is_whole_when_its_crcs_hold_and_it_has_a_tiff() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("t.zip");
        std::fs::write(&p, zip(&[("t.tif", b"tiff bytes"), ("doc.pdf", b"pdf")])).unwrap();
        assert_eq!(zip_bad(&p), None);
        std::fs::write(&p, zip(&[("doc.pdf", b"pdf")])).unwrap();
        assert_eq!(zip_bad(&p).as_deref(), Some("no GeoTIFF in it"));
        // A byte of the member's data changed: its CRC fails.
        let mut z = zip(&[("t.tif", &[7u8; 4000])]);
        let at = 30 + 5 + 3;
        z[at] ^= 1;
        std::fs::write(&p, &z).unwrap();
        assert!(zip_bad(&p).is_some_and(|w| w.starts_with("t.tif")), "{:?}", zip_bad(&p));
        std::fs::write(&p, b"PK no").unwrap();
        assert!(zip_bad(&p).is_some_and(|w| w.starts_with("not a whole zip")));
    }

    #[test]
    fn a_window_is_the_tiles_pixels_under_the_box() {
        // A 4 × 4 tile of 0.25° pixels from (10°E, 50°N): the box takes its middle 2 × 2, a
        // nodata pixel among them 0.
        let gt = [10.0, 0.25, 0.0, 50.0, 0.0, -0.25];
        let vals: Vec<f32> = (0..16).map(|v| v as f32).collect();
        let tif = crate::geotiff::write_f32(4, 4, &vals, 16, gt, 4326, Some(5.0)).unwrap();
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("g.zip"), zip(&[("g.tif", &tif)])).unwrap();
        let tiles = vec![("g.zip".to_string(), 0, [10.0, 49.0, 11.0, 50.0])];
        let (frames, used) = windows(d.path(), &tiles, [10.3, 49.3, 10.7, 49.7]).unwrap();
        assert_eq!(used, ["g.zip"]);
        let f = &frames[0];
        assert_eq!((f.header["col_off"].as_i64(), f.header["width"].as_i64()), (Some(1), Some(2)));
        let px: Vec<f32> = bytemuck::pod_collect_to_vec(&f.cols["data"]);
        assert_eq!(px, [0.0, 6.0, 9.0, 10.0]);
        // A tile away from the box: none.
        assert!(windows(d.path(), &tiles, [20.0, 49.3, 20.7, 49.7]).unwrap().0.is_empty());
    }
}
