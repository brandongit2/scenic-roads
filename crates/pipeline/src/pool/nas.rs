//! The NAS as the pool's protocol sees it (docs/pool.md §3): the six operations it needs, each
//! saying what the SMB share guarantees and what it doesn't, so the protocol runs the same against
//! the share (`Share`) and against the simulator's model of its faults. Paths are relative to the
//! NAS project folder, parts separated by `/`.

use anyhow::{ensure, Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};

/// What a create-new did.
#[derive(Debug)]
pub enum Created {
    /// This call made the file, and its bytes landed.
    Made,
    /// Something had that name: nothing was done.
    There,
    /// This call made the file, but its bytes didn't all land (the share went away between the
    /// two): the file is this Mac's, empty or short, for it to finish with a whole write (no other
    /// Mac's create of that name can succeed), or to leave; why.
    Unwritten(anyhow::Error),
}

/// The operations the pool's protocol makes on the NAS.
pub trait Nas {
    /// Makes `path` holding `bytes` unless something has that name. The create is atomic on the
    /// server (of two Macs' creates of one name, one wins); the bytes follow it, so another Mac
    /// may read the file empty or short meanwhile, and for good if this one stopped between the
    /// two. An error when it can't be said whether the file was made (its answer lost).
    fn create_new(&self, path: &str, bytes: &[u8]) -> Result<Created>;

    /// Writes `path` whole: a temporary name, renamed over it, the rename tried again for a few
    /// seconds while another Mac has the file open (crate::whole::rename_over). The rename may land
    /// long after the call began (this Mac asleep in between), over whatever was written there
    /// meanwhile; and it may have landed when the call says it failed (its answer lost).
    fn write_whole(&self, path: &str, bytes: &[u8]) -> Result<()>;

    /// The bytes of `path`; None when there's no such file. Possibly an older version than another
    /// Mac wrote last (macOS caches what it read), or a file being made: empty or short.
    fn read(&self, path: &str) -> Result<Option<Vec<u8>>>;

    /// Whether `path` exists: a stat, never a listing; stale as reads are.
    fn exists(&self, path: &str) -> Result<bool>;

    /// The names in folder `dir`, sorted, temporary files left out; none when it isn't there. Slow
    /// (3 to 33 s a folder under load) and possibly stale: for taking up a term and for GC, never
    /// in a loop.
    fn list(&self, dir: &str) -> Result<Vec<String>>;

    /// Removes file `path`; nothing when it isn't there.
    fn remove(&self, path: &str) -> Result<()>;
}

/// The NAS project folder (or a local folder standing in for it).
#[derive(Clone, Debug)]
pub struct Share {
    root: PathBuf,
}

impl Share {
    /// The share at `root`, the NAS project folder.
    pub fn new(root: &Path) -> Share {
        Share { root: root.to_path_buf() }
    }

    /// The folder the paths are under.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn at(&self, path: &str) -> PathBuf {
        self.root.join(path)
    }

    fn parent_made(p: &Path) -> Result<()> {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).with_context(|| format!("make {}", d.display()))?;
        }
        Ok(())
    }
}

impl Nas for Share {
    fn create_new(&self, path: &str, bytes: &[u8]) -> Result<Created> {
        let p = self.at(path);
        Share::parent_made(&p)?;
        let mut f = match std::fs::OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(Created::There),
            Err(e) => return Err(e).with_context(|| format!("create {}", p.display())),
        };
        let r = (|| -> Result<()> {
            f.write_all(bytes)?;
            f.sync_all()?;
            let n = f.metadata()?.len();
            ensure!(n == bytes.len() as u64, "{n} of {} bytes written", bytes.len());
            Ok(())
        })();
        Ok(match r {
            Ok(()) => Created::Made,
            Err(e) => Created::Unwritten(e.context(format!("write {} (made, and left short)", p.display()))),
        })
    }

    fn write_whole(&self, path: &str, bytes: &[u8]) -> Result<()> {
        let p = self.at(path);
        Share::parent_made(&p)?;
        crate::whole::write(&p, bytes)
    }

    fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let p = self.at(path);
        match std::fs::read(&p) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("read {}", p.display())),
        }
    }

    fn exists(&self, path: &str) -> Result<bool> {
        let p = self.at(path);
        p.try_exists().with_context(|| format!("stat {}", p.display()))
    }

    fn list(&self, dir: &str) -> Result<Vec<String>> {
        let d = self.at(dir);
        let rd = match std::fs::read_dir(&d) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("list {}", d.display())),
        };
        let mut names = Vec::new();
        // (An error midway is an error, not a shorter list.)
        for e in rd {
            let name = e.with_context(|| format!("list {}", d.display()))?.file_name().to_string_lossy().into_owned();
            if !crate::whole::is_tmp(Path::new(&name)) {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }

    fn remove(&self, path: &str) -> Result<()> {
        let p = self.at(path);
        match std::fs::remove_file(&p) {
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
        let names: std::collections::BTreeSet<String> = self.0.borrow().keys().filter_map(|k| k.strip_prefix(&prefix)).filter_map(|rest| rest.split('/').next()).map(str::to_string).collect();
        Ok(names.into_iter().collect())
    }

    fn remove(&self, path: &str) -> Result<()> {
        self.0.borrow_mut().remove(path);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_share_makes_once_writes_whole_and_lists_without_temporary_files() {
        let d = tempfile::tempdir().unwrap();
        let s = Share::new(d.path());
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
