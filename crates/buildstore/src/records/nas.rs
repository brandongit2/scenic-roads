//! The NAS as the pool's protocol sees it (docs/pool.md §3): the six operations it needs, each
//! saying what the SMB share guarantees and what it doesn't, so the protocol runs the same against
//! the share (`NasRoot`) and against the simulator's model of its faults. Paths are relative to the
//! NAS project folder, parts separated by `/`. (In the build's files' crate since task #916 step 6a,
//! docs/crates.md §6: `Out` and the map server read the records through it; the pool, at
//! agentcore::pool::nas, writes them.)

use anyhow::{Context, Result};
use store::nasfs::NasRoot;

pub use store::nasfs::Created;

/// Whether `e` is a call past its deadline (or refused, every thread for it stuck): pipeline's
/// agent::nasread's deadlines, buildstore::nasssh's.
pub fn timed_out(e: &anyhow::Error) -> bool {
    e.downcast_ref::<std::io::Error>().is_some_and(|e| e.kind() == std::io::ErrorKind::TimedOut)
}

/// The operations the pool's protocol makes on the NAS.
pub trait Nas {
    /// Makes `path` holding `bytes` unless something has that name. The create is atomic on the
    /// server (of two Macs' creates of one name, one wins); the bytes follow it, so another Mac
    /// may read the file empty or short meanwhile, and for good if this one stopped between the
    /// two. An error when it can't be said whether the file was made (its answer lost).
    fn create_new(&self, path: &str, bytes: &[u8]) -> Result<Created>;

    /// Writes `path` whole: a temporary name, renamed over it; when that rename stays busy (the
    /// server holding the old file open for another session, maybe for hours) the old file is
    /// renamed aside first (store::whole::write). The rename may land long after the call began
    /// (this Mac asleep in between), over whatever was written there meanwhile; and it may have
    /// landed when the call says it failed (its answer lost). `read` waits out the moment between.
    fn write_whole(&self, path: &str, bytes: &[u8]) -> Result<()>;

    /// The bytes of `path`; None when there's no such file. Possibly an older version than another
    /// Mac wrote last (macOS caches what it read), or a file being made: empty or short.
    fn read(&self, path: &str) -> Result<Option<Vec<u8>>>;

    /// `read`, asking past this Mac's cache (on a share: the file opened with `F_NOCACHE`, so its
    /// bytes come from the server, not the unified buffer cache). macOS's SMB client may still
    /// answer from an SMB lease it holds on the file: no promise beyond the page cache.
    fn read_fresh(&self, path: &str) -> Result<Option<Vec<u8>>> {
        self.read(path)
    }

    /// Whether `path` exists: a stat, never a listing; stale as reads are.
    fn exists(&self, path: &str) -> Result<bool>;
}

/// The NAS project folder: the protocol's operations are store::nasfs's (its `made` set and whole
/// writes in one place).
impl Nas for NasRoot {
    fn create_new(&self, path: &str, bytes: &[u8]) -> Result<Created> {
        self.join(path).create_new(bytes)
    }

    fn write_whole(&self, path: &str, bytes: &[u8]) -> Result<()> {
        self.join(path).write_whole_made(bytes)
    }

    fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let p = self.join(path);
        match p.read() {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("read {}", p.display())),
        }
    }

    fn read_fresh(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let p = self.join(path);
        match p.read_fresh() {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("read {}", p.display())),
        }
    }

    fn exists(&self, path: &str) -> Result<bool> {
        let p = self.join(path);
        p.try_exists().with_context(|| format!("stat {}", p.display()))
    }
}

/// The share read only, through a reader's own read of a path (the map server's, through its pool):
/// `read` answers None for a file that isn't there. Writes are refused (a reader of the records
/// reads the one path, super::now).
pub struct ReadOnly<'a>(pub &'a dyn Fn(&str) -> Result<Option<Vec<u8>>>);

impl Nas for ReadOnly<'_> {
    fn create_new(&self, path: &str, _: &[u8]) -> Result<Created> {
        anyhow::bail!("{path}: read only")
    }
    fn write_whole(&self, path: &str, _: &[u8]) -> Result<()> {
        anyhow::bail!("{path}: read only")
    }
    fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
        (self.0)(path)
    }
    fn exists(&self, path: &str) -> Result<bool> {
        Ok((self.0)(path)?.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_share_makes_once_and_writes_whole() {
        let d = tempfile::tempdir().unwrap();
        let s = NasRoot::local_for_test(d.path());
        assert!(matches!(s.create_new("state/build/lead-member.json", b"one").unwrap(), Created::Made));
        assert!(matches!(s.create_new("state/build/lead-member.json", b"two").unwrap(), Created::There), "made once");
        assert_eq!(s.read("state/build/lead-member.json").unwrap().as_deref(), Some(&b"one"[..]));
        s.write_whole("state/pool/members/m-1.json", b"a").unwrap();
        s.write_whole("state/pool/members/m-1.json", b"bb").unwrap();
        assert_eq!(s.read("state/pool/members/m-1.json").unwrap().as_deref(), Some(&b"bb"[..]));
        assert!(s.exists("state/build/lead-member.json").unwrap() && !s.exists("state/build/lead.json").unwrap());
        assert_eq!(s.read("state/build/lead.json").unwrap(), None);
    }
}
