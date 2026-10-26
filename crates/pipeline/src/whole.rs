//! Files checked whole: a PNG to its last chunk, a TIFF's strips or tiles inside the file, so a
//! copy cut short is fetched again rather than read for good. (Writing whole: store::whole.)

use std::path::Path;

/// Whether `b` is a PNG to its end: the signature, then chunks each inside it with its CRC right,
/// up to IEND.
pub fn png_whole(b: &[u8]) -> bool {
    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if !b.starts_with(SIG) {
        return false;
    }
    let mut p = SIG.len();
    while p + 12 <= b.len() {
        let len = u32::from_be_bytes([b[p], b[p + 1], b[p + 2], b[p + 3]]) as usize;
        let Some(end) = (p + 8).checked_add(len).filter(|&e| e + 4 <= b.len()) else { return false };
        if crc32fast::hash(&b[p + 4..end]) != u32::from_be_bytes([b[end], b[end + 1], b[end + 2], b[end + 3]]) {
            return false;
        }
        if &b[p + 4..p + 8] == b"IEND" {
            return true;
        }
        p = end + 4;
    }
    false
}

/// Whether a TIFF of `len` bytes, read through `at(offset, n)`, is whole: each of its images'
/// directories and strips or tiles inside the file (a file cut short loses its last ones, or the
/// directory GDAL writes last).
fn tiff_whole_with(len: u64, at: &dyn Fn(u64, usize) -> Option<Vec<u8>>) -> bool {
    let Some(h) = at(0, 16).or_else(|| at(0, 8)) else { return false };
    let le = match &h[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return false,
    };
    let num = |b: &[u8]| -> u64 {
        let mut v = 0u64;
        for i in 0..b.len() {
            let byte = if le { b[b.len() - 1 - i] } else { b[i] };
            v = v << 8 | byte as u64;
        }
        v
    };
    let big = match num(&h[2..4]) {
        42 => false,
        43 if h.len() >= 16 => true,
        _ => return false,
    };
    let (cnt_sz, ent_sz, off_sz) = if big { (8usize, 20usize, 8usize) } else { (2, 12, 4) };
    let mut ifd = if big { num(&h[8..16]) } else { num(&h[4..8]) };
    let mut images = 0;
    while ifd != 0 {
        images += 1;
        if images > 64 || ifd >= len {
            return false;
        }
        let Some(c) = at(ifd, cnt_sz) else { return false };
        let n = num(&c) as usize;
        let Some(ents) = n.checked_mul(ent_sz).and_then(|b| at(ifd + cnt_sz as u64, b + off_sz)) else { return false };
        // A tag's values: inline, or at the offset the entry gives.
        let values = |e: &[u8]| -> Option<Vec<u64>> {
            let sz = match num(&e[2..4]) {
                3 => 2,
                4 => 4,
                16 => 8,
                _ => return None,
            };
            let count = if big { num(&e[4..12]) } else { num(&e[4..8]) } as usize;
            let bytes = count.checked_mul(sz)?;
            let raw = if bytes <= off_sz {
                e[ent_sz - off_sz..ent_sz - off_sz + bytes].to_vec()
            } else {
                at(num(&e[ent_sz - off_sz..]), bytes)?
            };
            Some(raw.chunks_exact(sz).map(num).collect())
        };
        let (mut offs, mut lens) = (None, None);
        for e in ents[..n * ent_sz].chunks_exact(ent_sz) {
            match num(&e[..2]) {
                273 | 324 => offs = values(e),
                279 | 325 => lens = values(e),
                _ => {}
            }
        }
        let (Some(offs), Some(lens)) = (offs, lens) else { return false };
        if offs.len() != lens.len()
            || offs.iter().zip(&lens).any(|(&o, &l)| o.checked_add(l).is_none_or(|end| end > len))
        {
            return false;
        }
        ifd = num(&ents[n * ent_sz..]);
    }
    images > 0
}

/// Whether TIFF bytes `b` are whole.
pub fn tiff_bytes_whole(b: &[u8]) -> bool {
    tiff_whole_with(b.len() as u64, &|o, n| {
        let o = usize::try_from(o).ok()?;
        b.get(o..o.checked_add(n)?).map(<[u8]>::to_vec)
    })
}

/// Whether TIFF file `p` is whole (its directories read, not its data).
pub fn tiff_file_whole(p: &Path) -> bool {
    std::fs::File::open(p).is_ok_and(|f| tiff_whole(&f))
}

/// Whether open TIFF file `f` is whole (its directories read, not its data).
pub fn tiff_whole(f: &std::fs::File) -> bool {
    tiff_whole_checked(f).unwrap_or(false)
}

/// Whether TIFF file `p` is whole, or the I/O error that kept it from being read (a NAS's EIO or
/// timeout says nothing of the file: the store's reader names the error rather than calling a
/// whole file cut short, as on 25 Oct while the NAS restarted after a DSM update).
pub fn tiff_file_checked(p: &Path) -> std::io::Result<bool> {
    tiff_whole_checked(&std::fs::File::open(p)?)
}

/// Whether open TIFF file `f` is whole, or the first I/O error met reading it (a read cut by the
/// file's end is "not whole", not an error).
pub fn tiff_whole_checked(f: &std::fs::File) -> std::io::Result<bool> {
    use store::sys::PosIo;
    let len = f.metadata()?.len();
    let err = std::cell::RefCell::new(None);
    let whole = tiff_whole_with(len, &|o, n| {
        if o.checked_add(n as u64)? > len {
            return None;
        }
        let mut b = vec![0u8; n];
        match f.read_exact_at(&mut b, o) {
            Ok(()) => Some(b),
            Err(e) => {
                if e.kind() != std::io::ErrorKind::UnexpectedEof {
                    err.borrow_mut().get_or_insert(e);
                }
                None
            }
        }
    });
    match err.into_inner() {
        Some(e) => Err(e),
        None => Ok(whole),
    }
}

/// Whether kept file `p` is whole, by its kind (`.png`, `.tif`); a file of another kind is taken
/// as whole.
pub fn file_whole(p: &Path) -> bool {
    match p.extension().and_then(|x| x.to_str()) {
        Some("png") => std::fs::read(p).is_ok_and(|b| png_whole(&b)),
        Some("tif") => tiff_file_whole(p),
        _ => true,
    }
}

/// Small whole files for tests.
#[cfg(test)]
pub mod testfiles {
    /// A terrain PNG.
    pub fn png() -> Vec<u8> {
        roadcore::grid::encode_terrain_png(&vec![12.5f32; 256 * 256], 256, 256).unwrap()
    }

    /// A 4×2 u8 TIFF, little-endian, in two strips (or tiles) of four bytes after the header.
    pub fn tiff(tiled: bool) -> Vec<u8> {
        let mut b = b"II\x2a\x00".to_vec();
        b.extend_from_slice(&8u32.to_le_bytes());
        let data = 8 + 2 + 3 * 12 + 4;
        let (off_tag, len_tag) = if tiled { (324u16, 325u16) } else { (273u16, 279u16) };
        b.extend_from_slice(&3u16.to_le_bytes());
        let mut ent = |tag: u16, typ: u16, count: u32, v: u32| {
            b.extend_from_slice(&tag.to_le_bytes());
            b.extend_from_slice(&typ.to_le_bytes());
            b.extend_from_slice(&count.to_le_bytes());
            b.extend_from_slice(&v.to_le_bytes());
        };
        ent(256, 3, 1, 4);
        // Two offsets / counts don't fit inline: at the end, after the pixels.
        ent(off_tag, 4, 2, (data + 8) as u32);
        ent(len_tag, 4, 2, (data + 16) as u32);
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        for v in [data as u32, data as u32 + 4] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        for v in [4u32, 4] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b
    }
}

#[cfg(test)]
mod tests {
    use super::testfiles::png;
    use super::*;
    use std::path::PathBuf;

    fn tiff(dir: &Path, tiled: bool) -> PathBuf {
        let p = dir.join(if tiled { "t.tif" } else { "s.tif" });
        std::fs::write(&p, super::testfiles::tiff(tiled)).unwrap();
        p
    }

    #[test]
    fn pngs_whole_or_cut_short() {
        let b = png();
        assert!(png_whole(&b));
        assert!(!png_whole(&b[..b.len() - 1]), "IEND cut");
        assert!(!png_whole(&b[..b.len() / 2]), "cut in the data");
        let mut bad = b.clone();
        bad[b.len() / 2] ^= 1;
        assert!(!png_whole(&bad), "a flipped bit");
        assert!(!png_whole(b""));
    }

    #[test]
    fn tiff_read_errors_are_errors() {
        let d = tempfile::tempdir().unwrap();
        let p = tiff(d.path(), false);
        assert!(tiff_file_checked(&p).unwrap());
        std::fs::write(&p, &std::fs::read(&p).unwrap()[..9]).unwrap();
        assert!(!tiff_file_checked(&p).unwrap(), "cut short: not whole, no error");
        // A read that fails (a folder's read is EISDIR, as a NAS's EIO): an error, not "not whole".
        let e = tiff_whole_checked(&std::fs::File::open(d.path()).unwrap()).unwrap_err();
        assert_ne!(e.kind(), std::io::ErrorKind::UnexpectedEof, "{e}");
        assert!(tiff_file_checked(&d.path().join("missing.tif")).is_err());
    }

    #[test]
    fn tiffs_whole_or_cut_short() {
        let d = tempfile::tempdir().unwrap();
        for tiled in [false, true] {
            let p = tiff(d.path(), tiled);
            let b = std::fs::read(&p).unwrap();
            assert!(tiff_file_whole(&p) && tiff_bytes_whole(&b));
            // Cut at any point: not whole.
            for cut in [b.len() - 1, b.len() - 9, 20, 9, 3] {
                assert!(!tiff_bytes_whole(&b[..cut]), "cut at {cut}");
                std::fs::write(&p, &b[..cut]).unwrap();
                assert!(!tiff_file_whole(&p), "file cut at {cut}");
            }
            // A strip pointing past the end.
            let mut bad = b.clone();
            let n = bad.len();
            bad[n - 12..n - 8].copy_from_slice(&(n as u32).to_le_bytes());
            assert!(!tiff_bytes_whole(&bad));
        }
        assert!(!tiff_bytes_whole(b"not a tiff at all"));
    }

    #[test]
    fn written_whole_files_check_whole() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.png");
        store::whole::write(&p, &png()).unwrap();
        assert!(file_whole(&p));
    }
}
