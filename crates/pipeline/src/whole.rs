//! Files kept whole (docs/plan.md §3, Downloads). What's written to the NAS's stores, or copied from
//! them to a Mac's cache, goes by a temporary name (this Mac's name and the process's, so two Macs
//! never share one), is flushed to the disk and has its length checked before the rename: a write
//! cut short never takes the file's name. (The build agent's caches are made through
//! store::cachefile, the same way, and held.) And a kept file can be checked whole (a PNG to its last chunk, a TIFF's strips or tiles inside the file), so
//! a copy cut short is fetched again rather than read for good.

use anyhow::{ensure, Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};

/// A temporary name beside `path` (in WebAssembly, which has no processes, process 0's: a task's
/// folder is its own).
pub fn tmp_name(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!("{name}.{}.{}.tmp", crate::agent::cond::host(), store::sys::pid()))
}

/// Whether `p` is a temporary file (these, or another program's `.part`).
pub fn is_tmp(p: &Path) -> bool {
    p.extension().is_some_and(|x| x == "tmp" || x == "part" || x == "adopt")
}

/// Renames `from` over `to`. On the NAS's SMB share a rename over a file another Mac (or process)
/// has open fails as busy for a moment (EBUSY: a server reading the manifest, a job its keys): tried
/// again for a few seconds before it's an error.
pub fn rename_over(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        match std::fs::rename(from, to) {
            Err(e) if e.kind() == std::io::ErrorKind::ResourceBusy && tries < 20 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            r => return r,
        }
    }
}

/// Renames `from` over `to` (`rename_over`); when that stays busy, `to` is renamed aside first
/// (`aside_name`), `from` renamed into its free name, then asides removed (`Asides`). On the NAS's
/// SMB share a rename over a file another session holds open fails as busy for as long as it's
/// held, and the server may hold one for hours after the process that opened it is gone; a rename
/// of the held file away, and a rename into its free name, still go through, while removing it
/// fails until it's let go. Readers may find no `to` between the two renames, never a torn one.
/// When the second rename fails, the aside stays where it is (renaming it back could replace a
/// newer file): `to` is missing until the next write. Some(the aside) when the fallback was used.
pub fn rename_or_aside(from: &Path, to: &Path) -> std::io::Result<Option<PathBuf>> {
    static ASIDES: std::sync::Mutex<Asides> = std::sync::Mutex::new(Asides { held: Vec::new(), listed: None });
    aside_with(from, to, &rename_over, &|a, b| std::fs::rename(a, b), &|p| std::fs::remove_file(p), &ASIDES, std::time::Instant::now())
}

/// The asides not removed yet, at most this many remembered (the oldest forgotten; a listing finds
/// them again).
const ASIDES_KEPT: usize = 16;
/// At most this many removes a fallback.
const ASIDES_REMOVED: usize = 4;
/// A fallback lists the folder for asides (any Mac's, any process's) at most this often.
const ASIDES_LISTED: std::time::Duration = std::time::Duration::from_secs(60);

/// The asides a process knows of and hasn't removed (still held, or not tried yet), and when it last
/// listed for them.
pub struct Asides {
    held: Vec<PathBuf>,
    listed: Option<std::time::Instant>,
}

impl Asides {
    /// Takes in `aside` (just made) and, at most every `ASIDES_LISTED`, the folder's other asides of
    /// `to`; then removes up to `ASIDES_REMOVED` of them, the oldest first, keeping those that fail.
    fn sweep(&mut self, to: &Path, aside: &Path, remove: &dyn Fn(&Path) -> std::io::Result<()>, now: std::time::Instant) {
        if self.listed.is_none_or(|t| now.saturating_duration_since(t) >= ASIDES_LISTED) {
            self.listed = Some(now);
            let prefix = format!("{}.", to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            if let Some(Ok(rd)) = to.parent().map(std::fs::read_dir) {
                for e in rd.flatten() {
                    let n = e.file_name().to_string_lossy().into_owned();
                    let p = e.path();
                    if n.starts_with(&prefix) && n.ends_with(".aside.tmp") && p != aside && !self.held.contains(&p) {
                        self.held.push(p);
                    }
                }
            }
        }
        if !self.held.iter().any(|p| p == aside) {
            self.held.push(aside.to_path_buf());
        }
        let mut tried = 0;
        self.held.retain(|p| {
            if tried == ASIDES_REMOVED {
                return true;
            }
            tried += 1;
            remove(p).is_err_and(|e| e.kind() != std::io::ErrorKind::NotFound)
        });
        let over = self.held.len().saturating_sub(ASIDES_KEPT);
        self.held.drain(..over);
    }
}

/// A name to rename `path` aside to: its own, this Mac's and process's, a count, `.aside.tmp`.
fn aside_name(path: &Path) -> PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!("{name}.{}.{}.{n}.aside.tmp", crate::agent::cond::host(), store::sys::pid()))
}

type Rename<'a> = &'a dyn Fn(&Path, &Path) -> std::io::Result<()>;

/// `rename_or_aside` with its renames (`over`: the one tried again; `raw`: once), removal, asides
/// and clock given.
fn aside_with(from: &Path, to: &Path, over: Rename, raw: Rename, remove: &dyn Fn(&Path) -> std::io::Result<()>, asides: &std::sync::Mutex<Asides>, now: std::time::Instant) -> std::io::Result<Option<PathBuf>> {
    match over(from, to) {
        Err(e) if e.kind() == std::io::ErrorKind::ResourceBusy => {}
        r => return r.map(|()| None),
    }
    let aside = aside_name(to);
    raw(to, &aside)?;
    let r = over(from, to);
    let mut a = asides.lock().unwrap();
    a.sweep(to, &aside, remove, now);
    eprintln!("whole: rename over {} busy: the old file renamed aside{} ({} aside(s) not removed yet)", to.display(), if r.is_ok() { ", the new one put in its place" } else { "; the new one's rename failed too" }, a.held.len());
    r.map(|()| Some(aside))
}

/// Writes `b` as `path`, whole.
pub fn write(path: &Path, b: &[u8]) -> Result<()> {
    write_by(path, b, &rename_over)
}

/// Writes `b` as `path`, whole, its rename over `path` falling back to renaming `path` aside when
/// that stays busy (`rename_or_aside`): for a file only one writer replaces, whose readers take a
/// moment without it in their stride.
pub fn write_aside(path: &Path, b: &[u8]) -> Result<()> {
    write_by(path, b, &|f, t| rename_or_aside(f, t).map(drop))
}

fn write_by(path: &Path, b: &[u8], rename: Rename) -> Result<()> {
    let tmp = tmp_name(path);
    let r = (|| -> Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(b)?;
        f.sync_all()?;
        drop(f);
        let n = std::fs::metadata(&tmp)?.len();
        ensure!(n == b.len() as u64, "{n} of {} bytes written", b.len());
        rename(&tmp, path)?;
        Ok(())
    })();
    if r.is_err() {
        std::fs::remove_file(&tmp).ok();
    }
    r.with_context(|| format!("write {}", path.display()))
}

/// Copies `src` to `dst`, whole; the bytes copied.
pub fn copy(src: &Path, dst: &Path) -> Result<u64> {
    let tmp = tmp_name(dst);
    let r = (|| -> Result<u64> {
        let mut from = std::fs::File::open(src)?;
        let want = from.metadata()?.len();
        let mut to = std::fs::File::create(&tmp)?;
        let n = std::io::copy(&mut from, &mut to)?;
        to.sync_all()?;
        drop(to);
        let got = std::fs::metadata(&tmp)?.len();
        ensure!(n == want && got == want, "{got} of {want} bytes copied");
        rename_over(&tmp, dst)?;
        Ok(want)
    })();
    if r.is_err() {
        std::fs::remove_file(&tmp).ok();
    }
    r.with_context(|| format!("copy {} to {}", src.display(), dst.display()))
}

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
            let raw = if bytes <= off_sz { e[ent_sz - off_sz..ent_sz - off_sz + bytes].to_vec() } else { at(num(&e[ent_sz - off_sz..]), bytes)? };
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
        if offs.len() != lens.len() || offs.iter().zip(&lens).any(|(&o, &l)| o.checked_add(l).is_none_or(|end| end > len)) {
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
    use store::sys::PosIo;
    let Ok(len) = f.metadata().map(|m| m.len()) else { return false };
    tiff_whole_with(len, &|o, n| {
        if o.checked_add(n as u64)? > len {
            return None;
        }
        let mut b = vec![0u8; n];
        f.read_exact_at(&mut b, o).ok().map(|_| b)
    })
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

    fn tiff(dir: &Path, tiled: bool) -> PathBuf {
        let p = dir.join(if tiled { "t.tif" } else { "s.tif" });
        std::fs::write(&p, super::testfiles::tiff(tiled)).unwrap();
        p
    }

    /// A rename over a file held busy for good (as the NAS holds one another session left open)
    /// renames the held file aside and puts the new one in its place; the aside is removed, once
    /// it's no longer held, at a later fallback.
    #[test]
    fn a_rename_over_a_held_file_renames_it_aside() {
        use std::cell::RefCell;
        let d = tempfile::tempdir().unwrap();
        let to = d.path().join("m-1.json");
        std::fs::write(&to, b"old").unwrap();
        let held: RefCell<Vec<PathBuf>> = RefCell::new(vec![to.clone()]);
        let busy = || std::io::Error::from(std::io::ErrorKind::ResourceBusy);
        let over = |f: &Path, t: &Path| if held.borrow().iter().any(|p| p == t) { Err(busy()) } else { std::fs::rename(f, t) };
        let raw = |f: &Path, t: &Path| {
            let mut h = held.borrow_mut();
            if let Some(i) = h.iter().position(|p| p == f) {
                h[i] = t.to_path_buf(); // (the handle follows the file)
            }
            std::fs::rename(f, t)
        };
        let remove = |p: &Path| if held.borrow().iter().any(|h| h == p) { Err(busy()) } else { std::fs::remove_file(p) };
        let asides = std::sync::Mutex::new(Asides { held: Vec::new(), listed: None });
        let t0 = std::time::Instant::now();
        let tmp = d.path().join("a.tmp");
        std::fs::write(&tmp, b"new").unwrap();
        let aside = aside_with(&tmp, &to, &over, &raw, &remove, &asides, t0).unwrap().unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"new");
        assert_eq!(std::fs::read(&aside).unwrap(), b"old", "held, not removed");
        assert!(is_tmp(&aside));
        assert_eq!(asides.lock().unwrap().held, [aside.clone()]);
        // The next write isn't busy: a plain rename, the aside left for the next fallback.
        std::fs::write(&tmp, b"newer").unwrap();
        assert_eq!(aside_with(&tmp, &to, &over, &raw, &remove, &asides, t0).unwrap(), None);
        assert_eq!(std::fs::read(&to).unwrap(), b"newer");
        // Held again, the first aside let go: the fallback removes it.
        *held.borrow_mut() = vec![to.clone()];
        std::fs::write(&tmp, b"third").unwrap();
        let aside2 = aside_with(&tmp, &to, &over, &raw, &remove, &asides, t0).unwrap().unwrap();
        assert!(!aside.exists(), "freed aside removed");
        assert_eq!(asides.lock().unwrap().held, [aside2]);
        assert_eq!(std::fs::read(&to).unwrap(), b"third");
        // Another error isn't busy: no fallback.
        let gone = d.path().join("nope.tmp");
        assert!(aside_with(&gone, &d.path().join("x.json"), &|f, t| std::fs::rename(f, t), &raw, &remove, &asides, t0).is_err());
        assert!(!d.path().join("x.json").exists());
    }

    /// The second rename failing leaves the aside where it is (renaming it back could replace a
    /// newer file someone wrote meanwhile).
    #[test]
    fn a_failed_rename_after_the_aside_leaves_it() {
        let d = tempfile::tempdir().unwrap();
        let to = d.path().join("m.json");
        std::fs::write(&to, b"old").unwrap();
        let tries = std::cell::Cell::new(0);
        let over = |_: &Path, _: &Path| {
            tries.set(tries.get() + 1);
            Err(std::io::Error::from(if tries.get() == 1 { std::io::ErrorKind::ResourceBusy } else { std::io::ErrorKind::PermissionDenied }))
        };
        let raw = |f: &Path, t: &Path| {
            std::fs::rename(f, t)?;
            std::fs::write(f, b"newer").unwrap(); // (another write lands in between)
            Ok(())
        };
        let asides = std::sync::Mutex::new(Asides { held: Vec::new(), listed: None });
        let busy = |_: &Path| Err(std::io::Error::from(std::io::ErrorKind::ResourceBusy));
        assert!(aside_with(Path::new("x"), &to, &over, &raw, &busy, &asides, std::time::Instant::now()).is_err());
        assert_eq!(std::fs::read(&to).unwrap(), b"newer", "not replaced");
    }

    /// A fallback finds an earlier process's asides of the same file by a listing, at most once a
    /// minute, removes at most four a time, and remembers at most sixteen.
    #[test]
    fn asides_are_swept_bounded() {
        let d = tempfile::tempdir().unwrap();
        let to = d.path().join("m.json");
        for i in 0..30 {
            std::fs::write(d.path().join(format!("m.json.mac.{i}.0.aside.tmp")), b"x").unwrap();
        }
        std::fs::write(d.path().join("n.json.mac.1.0.aside.tmp"), b"x").unwrap();
        let count = || std::fs::read_dir(d.path()).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with("m.json.")).count();
        let mut a = Asides { held: Vec::new(), listed: None };
        let t0 = std::time::Instant::now();
        let mine = d.path().join("m.json.me.1.0.aside.tmp");
        std::fs::write(&mine, b"x").unwrap();
        let rm = |p: &Path| std::fs::remove_file(p);
        a.sweep(&to, &mine, &rm, t0);
        assert_eq!(count(), 31 - 4, "four removed");
        assert_eq!(a.held.len(), ASIDES_KEPT, "kept bounded");
        // Within the minute: no listing, four more of those remembered.
        a.sweep(&to, &mine, &rm, t0 + std::time::Duration::from_secs(10));
        assert_eq!(count(), 31 - 8);
        // A minute on: listed again, the forgotten ones found.
        for i in 0..10 {
            a.sweep(&to, &mine, &rm, t0 + std::time::Duration::from_secs(70 * (i + 1)));
        }
        assert_eq!(count(), 0, "all removed in the end");
        assert!(a.held.is_empty());
        assert!(d.path().join("n.json.mac.1.0.aside.tmp").exists(), "another file's asides left");
    }

    #[test]
    fn write_aside_writes_whole() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("m.json");
        write_aside(&p, b"a").unwrap();
        write_aside(&p, b"bb").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"bb");
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1, "no temporary file left");
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
    fn writes_and_copies_whole() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.png");
        write(&p, &png()).unwrap();
        assert!(file_whole(&p));
        let q = d.path().join("b.png");
        assert_eq!(copy(&p, &q).unwrap(), std::fs::metadata(&p).unwrap().len());
        assert_eq!(std::fs::read(&p).unwrap(), std::fs::read(&q).unwrap());
        // No temporary files left.
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 2);
        assert!(is_tmp(&tmp_name(&p)) && !is_tmp(&p));
        // A copy from nothing fails, and leaves nothing.
        assert!(copy(&d.path().join("none.png"), &d.path().join("c.png")).is_err());
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 2);
    }
}
