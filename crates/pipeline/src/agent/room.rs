//! Room on the build Mac's disk (docs/plan.md §8): before a job starts, while the disk's free
//! space is under `RESERVE`, the caches that are cheap to fill again lose their least recently
//! used files: Meta's canopy squares (`chm10/`, ~2 GB a 10° square, downloaded again in a minute
//! or so; scenic-metrics marks a square used when it reads it) and AWS's raw terrain tiles
//! (`aws-terrarium/`, read once per terrain run). The rest of the cache (the DEM samples, the
//! units' scenic results) is never deleted here.

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The free space a job starts with, at least, when the caches can make it.
pub const RESERVE: u64 = 60 << 30;

/// The caches' folders that may be emptied, under the agent's cache.
const CHEAP: [&str; 2] = ["chm10", "aws-terrarium"];

/// Frees up to `reserve` of free space at `cache` from the cheap caches, oldest use first; the
/// bytes deleted.
pub fn make_room(cache: &Path, reserve: u64) -> Result<u64> {
    make_room_with(cache, reserve, &disk_free)
}

fn make_room_with(cache: &Path, reserve: u64, free_space: &dyn Fn(&Path) -> std::io::Result<u64>) -> Result<u64> {
    let free = free_space(cache)?;
    if free >= reserve {
        return Ok(0);
    }
    let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
    for d in CHEAP {
        walk(&cache.join(d), &mut files);
    }
    files.sort();
    let need = reserve - free;
    let mut freed = 0u64;
    for (_, len, p) in files {
        if freed >= need {
            break;
        }
        if std::fs::remove_file(&p).is_ok() {
            freed += len;
        }
    }
    Ok(freed)
}

fn walk(dir: &Path, out: &mut Vec<(SystemTime, u64, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(md) = e.metadata() else { continue };
        if md.is_dir() {
            walk(&e.path(), out);
        } else {
            out.push((md.modified().unwrap_or(SystemTime::UNIX_EPOCH), md.len(), e.path()));
        }
    }
}

/// The free space on the disk holding `path` (a local disk).
pub fn disk_free(path: &Path) -> std::io::Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: `statfs` is plain old data; the path is NUL-terminated.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    #[allow(clippy::unnecessary_cast)]
    Ok((st.f_bavail as u64).saturating_mul(st.f_bsize as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn file(p: &Path, len: usize, age_s: u64) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, vec![0u8; len]).unwrap();
        let f = std::fs::File::options().append(true).open(p).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(age_s)).unwrap();
    }

    #[test]
    fn the_least_recently_used_cheap_files_go_first() {
        let d = tempfile::tempdir().unwrap();
        let c = d.path();
        file(&c.join("chm10/old.tif"), 100, 3000);
        file(&c.join("aws-terrarium/12/1/2.png"), 100, 2000);
        file(&c.join("chm10/new.tif"), 100, 10);
        file(&c.join("dem-units/6-1-2.dem"), 100, 9000);
        // 150 bytes short: the two oldest cheap files go, the DEM samples never.
        let freed = make_room_with(c, 1000, &|_| Ok(850)).unwrap();
        assert_eq!(freed, 200);
        assert!(!c.join("chm10/old.tif").exists() && !c.join("aws-terrarium/12/1/2.png").exists());
        assert!(c.join("chm10/new.tif").exists() && c.join("dem-units/6-1-2.dem").exists());
        // Room enough: nothing goes.
        assert_eq!(make_room_with(c, 1000, &|_| Ok(5000)).unwrap(), 0);
        assert!(disk_free(c).unwrap() > 0);
    }
}
