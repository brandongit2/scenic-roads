//! The mirror's reserve (docs/macs.md, "Mirror, per Mac"): the free space this Mac's downloads
//! leave on its disk, the owner's setting in the Regions panel. Kept in `<home>/reserve.json`
//! (`{"gb": 30}`, GB of 10⁹ bytes; written through a temporary file), read at start; 30 GB while
//! unset. Changed live: the mirror checks against it from its next round, and the mirror thread is
//! woken so downloads waiting for room may start. Raising it past what's free deletes nothing:
//! downloads just wait. The mirror keeps the larger of this and the build agent's room target
//! (`Mirror::set_floor`, set in crate::data).
//!
//!   PUT /api/downloads/reserve   {gb}: set it (whole GB, from 5 to less than the disk's size)

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const FILE: &str = "reserve.json";
/// The reserve while the owner hasn't set one (GB).
pub const DEFAULT_GB: u64 = 30;
/// The least it can be set to (GB): below that the disk fills to where macOS itself struggles.
pub const MIN_GB: u64 = 5;

#[derive(Serialize, Deserialize)]
struct Kept {
    gb: u64,
}

/// The reserve kept in `home` (GB): the default when there's none, or it doesn't read (logged).
pub fn load(home: &Path) -> u64 {
    let path = home.join(FILE);
    match std::fs::read(&path) {
        Ok(b) => match serde_json::from_slice::<Kept>(&b) {
            Ok(k) if k.gb >= MIN_GB => k.gb,
            Ok(k) => {
                eprintln!(
                    "reserve: {} says {} GB, under the least ({MIN_GB} GB); {DEFAULT_GB} GB",
                    path.display(),
                    k.gb
                );
                DEFAULT_GB
            }
            Err(e) => {
                eprintln!("reserve: {} doesn't read ({e}); {DEFAULT_GB} GB", path.display());
                DEFAULT_GB
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DEFAULT_GB,
        Err(e) => {
            eprintln!("reserve: can't read {}: {e}; {DEFAULT_GB} GB", path.display());
            DEFAULT_GB
        }
    }
}

/// Whether `gb` can be the reserve on a disk of `disk_bytes`: at least `MIN_GB`, less than the disk.
pub fn check(gb: u64, disk_bytes: u64) -> Result<()> {
    ensure!(gb >= MIN_GB, "The reserve is at least {MIN_GB} GB");
    let disk_gb = disk_bytes / 1_000_000_000;
    ensure!(gb < disk_gb, "The reserve must be less than the disk's size ({disk_gb} GB)");
    Ok(())
}

/// The setting in a server's home.
pub struct Setting {
    home: PathBuf,
    /// The setting (GB); held while it's saved and the mirror told, so two changes never cross.
    gb: Mutex<u64>,
}

impl Setting {
    pub fn load(home: &Path) -> Setting {
        Setting { home: home.to_owned(), gb: Mutex::new(load(home)) }
    }

    pub fn gb(&self) -> u64 {
        *self.gb.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The reserve (bytes).
    pub fn bytes(&self) -> u64 {
        self.gb() * 1_000_000_000
    }

    /// Sets it to `gb` on a disk of `disk_bytes` (`check`): kept, then `m` told.
    pub fn set(&self, gb: u64, disk_bytes: u64, m: &store::mirror::Mirror) -> Result<()> {
        check(gb, disk_bytes)?;
        let mut g = self.gb.lock().unwrap_or_else(|e| e.into_inner());
        save(&self.home, gb)?;
        *g = gb;
        m.set_reserve(gb * 1_000_000_000);
        Ok(())
    }
}

/// Keeps `gb` in `home` (through a temporary file, so a half-written file is never read).
pub fn save(home: &Path, gb: u64) -> Result<()> {
    let path = home.join(FILE);
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp = home.join(format!(
        "{FILE}.{}.{}.tmp",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::write(&tmp, serde_json::to_vec(&Kept { gb })?).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("rename {}", tmp.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_is_the_default_and_a_set_one_is_kept() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(load(home.path()), DEFAULT_GB);
        save(home.path(), 75).unwrap();
        assert_eq!(load(home.path()), 75);
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 1, "no temporary file left");
        save(home.path(), 12).unwrap();
        assert_eq!(load(home.path()), 12);
    }

    #[test]
    fn a_file_that_doesnt_read_or_is_too_low_is_the_default() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(FILE), b"{\"gb\": ").unwrap();
        assert_eq!(load(home.path()), DEFAULT_GB);
        std::fs::write(home.path().join(FILE), b"{\"gb\": 1}").unwrap();
        assert_eq!(load(home.path()), DEFAULT_GB);
    }

    #[test]
    fn bounds() {
        let disk = 500_000_000_000;
        assert!(check(MIN_GB, disk).is_ok());
        assert!(check(MIN_GB - 1, disk).is_err());
        assert!(check(0, disk).is_err());
        assert!(check(499, disk).is_ok());
        assert!(check(500, disk).is_err());
        assert!(check(10_000, disk).is_err());
    }
}
