//! The NAS as the pool's protocol sees it (docs/pool.md §3): the six operations it needs, each
//! saying what the SMB share guarantees and what it doesn't, so the protocol runs the same against
//! the share (`NasRoot`) and against the simulator's model of its faults. Paths are relative to the
//! NAS project folder, parts separated by `/`.

use anyhow::{Context, Result};
use store::nasfs::NasRoot;

pub use store::nasfs::Created;

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

    /// The names in folder `dir`, sorted, temporary files left out; none when it isn't there. Slow
    /// (3 to 33 s a folder under load) and possibly stale: for taking up a term and for GC, never
    /// in a loop.
    fn list(&self, dir: &str) -> Result<Vec<String>>;

    /// Removes file `path`; nothing when it isn't there.
    fn remove(&self, path: &str) -> Result<()>;
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

    fn list(&self, dir: &str) -> Result<Vec<String>> {
        let d = self.join(dir);
        d.list().with_context(|| format!("list {}", d.display()))
    }

    fn remove(&self, path: &str) -> Result<()> {
        let p = self.join(path);
        match p.remove_file() {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("remove {}", p.display())),
        }
    }
}

/// Whether bytes read of a file made with create-new aren't all there: empty, or JSON cut short (its
/// maker between its create and its bytes, or stopped there).
pub fn short(b: &[u8]) -> bool {
    b.is_empty() || serde_json::from_slice::<serde_json::Value>(b).err().is_some_and(|e| e.is_eof())
}

/// A NAS in memory without faults, for the modules' tests (the simulator has its own, with them).
#[cfg(test)]
#[derive(Default)]
pub struct Mem(pub std::cell::RefCell<std::collections::BTreeMap<String, Vec<u8>>>);

#[cfg(test)]
impl Nas for Mem {
    fn create_new(&self, path: &str, bytes: &[u8]) -> Result<Created> {
        let mut m = self.0.borrow_mut();
        if m.contains_key(path) {
            return Ok(Created::There);
        }
        m.insert(path.to_string(), bytes.to_vec());
        Ok(Created::Made)
    }

    fn write_whole(&self, path: &str, bytes: &[u8]) -> Result<()> {
        self.0.borrow_mut().insert(path.to_string(), bytes.to_vec());
        Ok(())
    }

    fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.0.borrow().get(path).cloned())
    }

    fn exists(&self, path: &str) -> Result<bool> {
        Ok(self.0.borrow().contains_key(path))
    }

    fn list(&self, dir: &str) -> Result<Vec<String>> {
        let prefix = format!("{dir}/");
        let names: std::collections::BTreeSet<String> = self
            .0
            .borrow()
            .keys()
            .filter_map(|k| k.strip_prefix(&prefix))
            .filter_map(|rest| rest.split('/').next())
            .map(str::to_string)
            .collect();
        Ok(names.into_iter().collect())
    }

    fn remove(&self, path: &str) -> Result<()> {
        self.0.borrow_mut().remove(path);
        Ok(())
    }
}

/// The share read only, through a reader's own read of a path (the map server's, through its pool):
/// `read` answers None for a file that isn't there. Writes and listings are refused (a reader of
/// the records lists nothing while the lead's hint is there: crate::pool::term::current).
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
    fn list(&self, dir: &str) -> Result<Vec<String>> {
        anyhow::bail!("{dir}: no listing through this read")
    }
    fn remove(&self, path: &str) -> Result<()> {
        anyhow::bail!("{path}: read only")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_share_makes_once_writes_whole_and_lists_without_temporary_files() {
        let d = tempfile::tempdir().unwrap();
        let s = NasRoot::local_for_test(d.path());
        assert!(matches!(s.create_new("state/build/terms/1.json", b"one").unwrap(), Created::Made));
        assert!(matches!(s.create_new("state/build/terms/1.json", b"two").unwrap(), Created::There), "made once");
        assert_eq!(s.read("state/build/terms/1.json").unwrap().as_deref(), Some(&b"one"[..]));
        s.write_whole("state/pool/members/m-1.json", b"a").unwrap();
        s.write_whole("state/pool/members/m-1.json", b"bb").unwrap();
        assert_eq!(s.read("state/pool/members/m-1.json").unwrap().as_deref(), Some(&b"bb"[..]));
        std::fs::write(d.path().join("state/pool/members/m-2.json.mac.7.tmp"), b"x").unwrap();
        assert_eq!(s.list("state/pool/members").unwrap(), ["m-1.json"]);
        assert_eq!(s.list("state/build").unwrap(), ["terms"]);
        assert!(s.list("state/journal").unwrap().is_empty(), "no folder: nothing");
        assert!(s.exists("state/build/terms/1.json").unwrap() && !s.exists("state/build/terms/2.json").unwrap());
        assert_eq!(s.read("state/build/terms/2.json").unwrap(), None);
        s.remove("state/pool/members/m-1.json").unwrap();
        s.remove("state/pool/members/m-1.json").unwrap();
        assert!(!s.exists("state/pool/members/m-1.json").unwrap());
    }
}
