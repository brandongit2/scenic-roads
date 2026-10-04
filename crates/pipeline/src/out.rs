//! Where build steps put their outputs: the NAS project folder (docs/plan.md §3), as immutable
//! content-named files, with a build manifest (logical name → file) the catalog is made from.
//!
//! A file is first written locally (the build Mac's SSD), hashed, then copied to `<name>.tmp` on
//! the NAS, read back past the client's cache and checked against its hash, and renamed into place
//! (`store::naming::write_atomic`). A file already there is reused and touched, so GC sees it as
//! in use. `verify` can also check uploads on the NAS itself (SHA-256 over SSH, run by hand).
//!
//! Several steps may run at once on the build Mac (the agent's job and a manual one): each records
//! its own changes and merges them into the manifest on disk under this Mac's lock (`BuildLock`)
//! when it saves. A helper's job on the other Mac (docs/plan.md §8, Two Macs; `SCENIC_HANDOFF`)
//! saves its changes as hand-offs instead (crate::handoff), which the build Mac's agent merges: the
//! build Mac alone writes the manifest.

use anyhow::{bail, Context, Result};
use sha2::Digest;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

pub struct Out {
    root: PathBuf,
    /// Logical name → content name of every file this build wrote or reused.
    pub manifest: BTreeMap<String, String>,
    /// This run's changes to the manifest (None: removed), merged into the file when saving.
    changes: BTreeMap<String, Option<String>>,
    /// Uploads this run checked (no longer pending), likewise.
    checked: std::collections::BTreeSet<String>,
    /// Content name → SHA-256 (hex) of uploads not yet verified on the NAS.
    pending: BTreeMap<String, String>,
    manifest_path: PathBuf,
    /// Local scratch space for files before upload.
    pub scratch: PathBuf,
    /// A helper's job: where its saves go as hand-offs ($SCENIC_HANDOFF), never the manifest.
    handoff: Option<PathBuf>,
}

/// A JSON record (the manifest, the unverified uploads): empty when there's none yet, an error when
/// it can't be read now (an SMB hiccup), so nothing is ever written from an empty one by mistake.
pub fn read_record<T: Default + serde::de::DeserializeOwned>(p: &Path) -> Result<T> {
    match std::fs::read(p) {
        Ok(b) => serde_json::from_slice(&b).with_context(|| format!("parse {}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e).with_context(|| format!("read {}", p.display())),
    }
}

fn sha256_file(p: &Path) -> Result<String> {
    let mut f = std::fs::File::open(p)?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 8 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}

impl Out {
    /// `root`: the NAS project folder (or a local folder standing in for it); `scratch`: local space.
    pub fn open(root: &Path, scratch: &Path) -> Result<Self> {
        std::fs::create_dir_all(scratch)?;
        let manifest_path = root.join("state/build/manifest.json");
        let manifest = read_record(&manifest_path)?;
        let handoff = std::env::var_os("SCENIC_HANDOFF").map(PathBuf::from);
        // (A helper's job hands off only its own uploads.)
        let pending = if handoff.is_some() { BTreeMap::new() } else { read_record(&root.join("state/build/pending.json"))? };
        Ok(Out { root: root.to_path_buf(), manifest, changes: BTreeMap::new(), checked: Default::default(), pending, manifest_path, scratch: scratch.to_path_buf(), handoff })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The NAS path of a content name.
    pub fn path(&self, content: &str) -> PathBuf {
        self.root.join(content)
    }

    /// The content name currently recorded for a logical name.
    pub fn get(&self, logical: &str) -> Option<&str> {
        self.manifest.get(logical).map(String::as_str)
    }

    /// A local scratch path for building a file before `put_file`.
    pub fn scratch_file(&self, name: &str) -> PathBuf {
        self.scratch.join(name.replace('/', "_"))
    }

    /// Upload a local file under `logical` (the local file is removed afterwards). Returns its
    /// content name. An identical file already on the NAS is reused (and touched), not copied again;
    /// one of the same name with another size is an error (content-named files are never rewritten).
    pub fn put_file(&mut self, logical: &str, ext: &str, local: &Path) -> Result<String> {
        if logical.contains('.') {
            bail!("logical names have no dots: {logical}");
        }
        let h = store::naming::hash16_file(local)?;
        let name = store::naming::content_name(logical, &h, ext);
        let dest = self.root.join(&name);
        let existed = dest.exists();
        let sha = if existed { None } else { Some(sha256_file(local)?) };
        let got = store::naming::write_atomic(&self.root, logical, ext, store::naming::Source::File(local))?;
        anyhow::ensure!(got == name, "{logical}: wrote {got}, expected {name}");
        if existed {
            // In use again: a fresh time keeps GC's age rule from taking it before a catalog does.
            if let Ok(f) = std::fs::File::options().write(true).open(&dest) {
                f.set_modified(std::time::SystemTime::now()).ok();
            }
        }
        if let Some(sha) = sha {
            self.pending.insert(name.clone(), sha);
        }
        std::fs::remove_file(local).ok();
        self.manifest.insert(logical.to_string(), name.clone());
        self.changes.insert(logical.to_string(), Some(name.clone()));
        Ok(name)
    }

    /// Upload bytes under `logical`.
    pub fn put_bytes(&mut self, logical: &str, ext: &str, bytes: &[u8]) -> Result<String> {
        let local = self.scratch_file(&format!("{logical}.{ext}"));
        std::fs::write(&local, bytes)?;
        self.put_file(logical, ext, &local)
    }

    /// Record a logical name as gone (its file stays until GC).
    pub fn remove(&mut self, logical: &str) {
        self.manifest.remove(logical);
        self.changes.insert(logical.to_string(), None);
    }

    /// Writes this run's changes into the manifest on disk (re-read under a lock, so another step's
    /// changes saved meanwhile are kept), and the unverified uploads likewise. A helper's job hands
    /// them off instead (crate::handoff).
    pub fn save(&mut self) -> Result<()> {
        if let Some(dir) = &self.handoff {
            if self.changes.is_empty() && self.pending.is_empty() && self.checked.is_empty() {
                return Ok(());
            }
            let h = crate::handoff::Handoff { changes: self.changes.clone(), pending: self.pending.clone(), checked: self.checked.iter().cloned().collect(), done: None };
            crate::handoff::write(dir, &h)?;
            self.changes.clear();
            self.pending.clear();
            self.checked.clear();
            return Ok(());
        }
        // The build Mac alone writes the records (docs/plan.md §8, Two Macs): a save from another
        // Mac (a step run by hand there, not as a helper's job) is refused, rather than let race it.
        // Its agent's jobs are the build Mac's (SCENIC_BUILD_MAC); one run by hand, by its name.
        if std::env::var_os("SCENIC_BUILD_MAC").is_none() {
            let host = crate::agent::cond::host();
            match std::fs::read_to_string(self.root.join("state/build/writer")) {
                Ok(w) if w.trim() != host => bail!("{} is the build Mac, which alone writes the build's records; this Mac ({host}) saves only as a helper's job (SCENIC_HANDOFF)", w.trim()),
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).context("read state/build/writer"),
            }
        }
        let lock = BuildLock::take(&self.root)?;
        self.save_held(&lock)
    }

    /// Takes in a hand-off's changes (the build Mac's agent merging them: crate::handoff::merge).
    pub fn absorb(&mut self, h: &crate::handoff::Handoff) {
        for (k, v) in &h.changes {
            match v {
                Some(n) => self.manifest.insert(k.clone(), n.clone()),
                None => self.manifest.remove(k),
            };
            self.changes.insert(k.clone(), v.clone());
        }
        self.pending.extend(h.pending.clone());
        self.checked.extend(h.checked.iter().cloned());
    }

    /// `save`, under this Mac's build lock, held.
    pub fn save_held(&mut self, _lock: &BuildLock) -> Result<()> {
        let dir = self.manifest_path.parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        let mut on_disk: BTreeMap<String, String> = read_record(&self.manifest_path)?;
        for (k, v) in &self.changes {
            match v {
                Some(n) => on_disk.insert(k.clone(), n.clone()),
                None => on_disk.remove(k),
            };
        }
        let pending_path = dir.join("pending.json");
        let mut pending: BTreeMap<String, String> = read_record(&pending_path)?;
        pending.extend(self.pending.clone());
        pending.retain(|k, _| !self.checked.contains(k));
        for (p, v) in [(&self.manifest_path, serde_json::to_vec_pretty(&on_disk)?), (&pending_path, serde_json::to_vec_pretty(&pending)?)] {
            let tmp = p.with_extension(format!("json.{}.tmp", std::process::id()));
            std::fs::write(&tmp, v)?;
            std::fs::rename(&tmp, p)?;
        }
        self.manifest = on_disk;
        self.pending = pending;
        self.changes.clear();
        self.checked.clear();
        Ok(())
    }

    /// Check every unverified upload on the NAS itself: SHA-256 computed there over SSH (`ssh`
    /// runs a command on the NAS; `nas_root` is the project folder as the NAS sees it). Files that
    /// don't match are deleted there (over SSH, so they skip the share's Recycle Bin) and an error
    /// lists them; rerunning the step uploads them again.
    pub fn verify(&mut self, ssh: &[&str], nas_root: &str) -> Result<usize> {
        if self.pending.is_empty() {
            return Ok(0);
        }
        let names: Vec<String> = self.pending.keys().cloned().collect();
        let mut bad = Vec::new();
        for chunk in names.chunks(200) {
            let mut cmd = std::process::Command::new(ssh[0]);
            cmd.args(&ssh[1..]);
            let quoted: Vec<String> = chunk.iter().map(|n| format!("'{}'", n.replace('\'', "'\\''"))).collect();
            cmd.arg(format!("cd '{nas_root}' && sha256sum {}", quoted.join(" ")));
            let out = cmd.output().context("ssh to the NAS")?;
            let text = String::from_utf8_lossy(&out.stdout);
            let mut got: BTreeMap<String, String> = BTreeMap::new();
            for line in text.lines() {
                if let Some((h, n)) = line.split_once("  ") {
                    got.insert(n.to_string(), h.to_string());
                }
            }
            for n in chunk {
                if got.get(n) != self.pending.get(n) {
                    bad.push(n.clone());
                }
            }
        }
        let ok = names.len() - bad.len();
        for n in &names {
            if !bad.contains(n) {
                self.pending.remove(n);
                self.checked.insert(n.clone());
            }
        }
        if !bad.is_empty() {
            let quoted: Vec<String> = bad.iter().map(|n| format!("'{}'", n.replace('\'', "'\\''"))).collect();
            let mut cmd = std::process::Command::new(ssh[0]);
            cmd.args(&ssh[1..]);
            cmd.arg(format!("cd '{nas_root}' && rm -f {}", quoted.join(" ")));
            cmd.status().ok();
            for n in &bad {
                self.pending.remove(n);
                self.checked.insert(n.clone());
                let gone: Vec<String> = self.manifest.iter().filter(|(_, v)| *v == n).map(|(k, _)| k.clone()).collect();
                for k in gone {
                    self.remove(&k);
                }
            }
            self.save()?;
            bail!("{} uploads failed verification and were removed: {}", bad.len(), bad.join(", "));
        }
        self.save()?;
        Ok(ok)
    }
}

/// An exclusive lock for saving a root's manifest among this Mac's processes, in a local file named
/// after the root: released when dropped.
pub struct BuildLock(#[allow(dead_code)] std::fs::File);

impl BuildLock {
    fn file(root: &Path) -> Result<std::fs::File> {
        let key = store::naming::hash16(root.to_string_lossy().as_bytes());
        let p = std::env::temp_dir().join(format!("scenic-build-{key}.lock"));
        std::fs::File::options().create(true).truncate(false).write(true).open(&p).with_context(|| format!("open {}", p.display()))
    }

    /// Waits until the lock is ours.
    pub fn take(root: &Path) -> Result<BuildLock> {
        use std::os::fd::AsRawFd;
        let f = Self::file(root)?;
        // SAFETY: flock on a descriptor we own; it blocks until the lock is ours.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error()).context("lock the build manifest");
        }
        Ok(BuildLock(f))
    }

    /// The lock when it's free now; None when another holds it (a paused job may, for hours).
    pub fn try_take(root: &Path) -> Result<Option<BuildLock>> {
        use std::os::fd::AsRawFd;
        let f = Self::file(root)?;
        // SAFETY: flock on a descriptor we own; it doesn't block.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
                return Ok(None);
            }
            return Err(e).context("lock the build manifest");
        }
        Ok(Some(BuildLock(f)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_unreadable_are_errors_not_empty() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("manifest.json");
        // None yet: empty.
        assert!(read_record::<BTreeMap<String, String>>(&p).unwrap().is_empty());
        // Unreadable (a folder in its place, as an I/O error), or not JSON: errors.
        std::fs::create_dir(&p).unwrap();
        assert!(read_record::<BTreeMap<String, String>>(&p).is_err());
        std::fs::remove_dir(&p).unwrap();
        std::fs::write(&p, b"{not json").unwrap();
        assert!(read_record::<BTreeMap<String, String>>(&p).is_err());
        // A save with the manifest unreadable fails, and writes nothing over it.
        let root = d.path().join("root");
        let mut out = Out::open(&root, &d.path().join("s")).unwrap();
        out.changes.insert("a".into(), Some("a.1111111111111111.x".into()));
        std::fs::create_dir_all(root.join("state/build/manifest.json")).unwrap();
        assert!(out.save().is_err());
        assert!(root.join("state/build/manifest.json").is_dir());
        // And opening with it unreadable fails too.
        assert!(Out::open(&root, &d.path().join("s")).is_err());
    }

    #[test]
    fn only_the_build_mac_saves_the_records() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("root");
        let mut out = Out::open(&root, &d.path().join("s")).unwrap();
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        // Another Mac named the writer: refused.
        std::fs::write(root.join("state/build/writer"), "not-this-mac").unwrap();
        out.changes.insert("a".into(), Some("a.1111111111111111.x".into()));
        assert!(out.save().is_err());
        assert!(!root.join("state/build/manifest.json").exists());
        // This Mac: saved.
        std::fs::write(root.join("state/build/writer"), crate::agent::cond::host()).unwrap();
        out.save().unwrap();
        assert!(root.join("state/build/manifest.json").exists());
    }
}
