//! Where build steps put their outputs: the NAS project folder (docs/plan.md §3), as immutable
//! content-named files, with a build manifest (logical name → file) the catalog is made from.
//!
//! A file is first written locally (the build Mac's SSD), hashed, then copied to `<name>.tmp` on
//! the NAS and renamed. The copy is verified on the NAS itself (SHA-256 over SSH, see `verify`)
//! before any catalog references it, so a corrupt upload is never served.

use anyhow::{bail, Context, Result};
use sha2::Digest;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub struct Out {
    root: PathBuf,
    /// Logical name → content name of every file this build wrote or reused.
    pub manifest: BTreeMap<String, String>,
    /// Content name → SHA-256 (hex) of uploads not yet verified on the NAS.
    pending: BTreeMap<String, String>,
    manifest_path: PathBuf,
    /// Local scratch space for files before upload.
    pub scratch: PathBuf,
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
        let manifest = match std::fs::read(&manifest_path) {
            Ok(b) => serde_json::from_slice(&b).context("state/build/manifest.json")?,
            Err(_) => BTreeMap::new(),
        };
        let pending_path = root.join("state/build/pending.json");
        let pending = match std::fs::read(&pending_path) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_default(),
            Err(_) => BTreeMap::new(),
        };
        Ok(Out { root: root.to_path_buf(), manifest, pending, manifest_path, scratch: scratch.to_path_buf() })
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
    /// content name. An identical file already on the NAS is reused, not copied again.
    pub fn put_file(&mut self, logical: &str, ext: &str, local: &Path) -> Result<String> {
        if logical.contains('.') {
            bail!("logical names have no dots: {logical}");
        }
        let h = store::naming::hash16_file(local)?;
        let name = store::naming::content_name(logical, &h, ext);
        let dest = self.root.join(&name);
        let size = std::fs::metadata(local)?.len();
        let exists = std::fs::metadata(&dest).map(|m| m.len() == size).unwrap_or(false);
        if !exists {
            let sha = sha256_file(local)?;
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let tmp = self.root.join(format!("{name}.tmp"));
            {
                let mut src = std::fs::File::open(local)?;
                let mut dst = std::fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
                std::io::copy(&mut src, &mut dst)?;
                dst.flush()?;
                dst.sync_all()?;
            }
            std::fs::rename(&tmp, &dest)?;
            self.pending.insert(name.clone(), sha);
        }
        std::fs::remove_file(local).ok();
        self.manifest.insert(logical.to_string(), name.clone());
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
    }

    pub fn save(&self) -> Result<()> {
        let dir = self.manifest_path.parent().unwrap();
        std::fs::create_dir_all(dir)?;
        for (p, v) in [(&self.manifest_path, serde_json::to_vec_pretty(&self.manifest)?), (&dir.join("pending.json"), serde_json::to_vec_pretty(&self.pending)?)] {
            let tmp = p.with_extension("json.tmp");
            std::fs::write(&tmp, v)?;
            std::fs::rename(&tmp, p)?;
        }
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
                self.manifest.retain(|_, v| v != n);
            }
            self.save()?;
            bail!("{} uploads failed verification and were removed: {}", bad.len(), bad.join(", "));
        }
        self.save()?;
        Ok(ok)
    }
}
