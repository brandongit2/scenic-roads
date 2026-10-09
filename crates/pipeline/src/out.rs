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
    /// Logical name → content name of every file this build wrote or reused (a round's job: with
    /// the units as they were when the round began, `as_of`).
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
    /// A round's job ($SCENIC_UNITS_AS_OF): the units' outputs as they were when the round began,
    /// read in place of the manifest's own (`units_as_of`).
    as_of: Option<BTreeMap<String, String>>,
}

/// A unit's outputs that map tiles, the road index and rail stops read: its base pack, road values
/// and roads' English (a prune of a unit drops them; its job also writes analysis grids, which
/// nothing a round makes reads).
pub const UNIT_OUTPUTS: [&str; 3] = ["base/", "global/roads/", "global/roaden/"];

/// What a round fixes as it begins (`units_as_of`): the units' outputs, and the 3D buildings' hi
/// packs, so a bldtiles job ending meanwhile doesn't change the round's catalog (made again and
/// again as their jobs end); they go out with the next.
pub const AS_OF_OUTPUTS: [&str; 4] = ["base/", "global/roads/", "global/roaden/", "layers/buildings/hi/"];

/// Names the file of a round under way and when it began, `<path>#<began>` (agent::build::Round, in
/// the agent's folder): a round's jobs (its map tiles, road index, rail stops and catalog) read the
/// units as they were when it began (`units_as_of`), so what's built meanwhile changes nothing the
/// round makes; it waits for the next.
pub const UNITS_AS_OF_ENV: &str = "SCENIC_UNITS_AS_OF";

/// `m` with its units' outputs and 3D buildings' packs (`AS_OF_OUTPUTS`) as `then` had them: those
/// built since left out, those rebuilt since as they were, those dropped since (a prune, or a
/// rebuild that left a unit no ways) still out.
pub fn units_as_of(m: &BTreeMap<String, String>, then: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    m.iter()
        .filter_map(|(l, c)| match AS_OF_OUTPUTS.iter().any(|p| l.starts_with(p)) {
            true => then.get(l).map(|t| (l.clone(), t.clone())),
            false => Some((l.clone(), c.clone())),
        })
        .collect()
}

/// The units of a round's file (`UNITS_AS_OF_ENV`: `<path>#<began>`): an error when it can't be
/// read, or it's another round's, or the round's over, so a round's job fails rather than make its
/// part from other units than the round's.
fn read_as_of(spec: &str) -> Result<BTreeMap<String, String>> {
    #[derive(serde::Deserialize)]
    struct Round {
        began: u64,
        units: BTreeMap<String, String>,
        #[serde(default)]
        over: bool,
    }
    let (p, began) = spec.rsplit_once('#').and_then(|(p, b)| Some((p, b.parse::<u64>().ok()?))).with_context(|| format!("{UNITS_AS_OF_ENV}={spec:?}: not <path>#<began>"))?;
    let b = std::fs::read(p).with_context(|| format!("read the round's units, {p}"))?;
    let r: Round = serde_json::from_slice(&b).with_context(|| format!("parse {p}"))?;
    anyhow::ensure!(r.began == began && !r.over, "{p} is no longer the round this job is for (it began at {began}; the file's began at {}{})", r.began, if r.over { ", and is over" } else { "" });
    Ok(r.units)
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
        store::naming::count_moved(n);
    }
    Ok(format!("{:x}", h.finalize()))
}

impl Out {
    /// `root`: the NAS project folder (or a local folder standing in for it); `scratch`: local space.
    pub fn open(root: &Path, scratch: &Path) -> Result<Self> {
        Self::open_as_of(root, scratch, std::env::var(UNITS_AS_OF_ENV).ok().as_deref())
    }

    /// `open`, a round's job's (`as_of`: its round's file and when it began, `UNITS_AS_OF_ENV`).
    pub fn open_as_of(root: &Path, scratch: &Path, as_of: Option<&str>) -> Result<Self> {
        let _p = crate::timings::phase("records read", crate::timings::Class::NasRead);
        std::fs::create_dir_all(scratch)?;
        let manifest_path = root.join("state/build/manifest.json");
        let as_of = as_of.map(read_as_of).transpose()?;
        let manifest: BTreeMap<String, String> = read_record(&manifest_path)?;
        let manifest = match &as_of {
            Some(then) => units_as_of(&manifest, then),
            None => manifest,
        };
        let handoff = std::env::var_os("SCENIC_HANDOFF").map(PathBuf::from);
        // (A helper's job hands off only its own uploads.)
        let pending = if handoff.is_some() { BTreeMap::new() } else { read_record(&root.join("state/build/pending.json"))? };
        Ok(Out { root: root.to_path_buf(), manifest, changes: BTreeMap::new(), checked: Default::default(), pending, manifest_path, scratch: scratch.to_path_buf(), handoff, as_of })
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
        // (Its bytes, to the phase it's uploaded in: crate::timings.)
        crate::timings::count(if existed { 0 } else { std::fs::metadata(local).map(|m| m.len()).unwrap_or(0) }, 1);
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

    /// `put_file`, saying how far it is to `on` about once a second, as (bytes, total): it reads the
    /// file four times (its name's hash, twice; its checksum; the copy) and the copy once more on the
    /// NAS, for a large file most of a step's time.
    pub fn put_file_with(&mut self, logical: &str, ext: &str, local: &Path, on: &(dyn Fn(u64, u64) + Sync)) -> Result<String> {
        let total = 5 * std::fs::metadata(local)?.len();
        let start = store::naming::moved();
        let stop = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|s| {
            let said = s.spawn(|| {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    on((store::naming::moved() - start).min(total), total);
                    std::thread::park_timeout(std::time::Duration::from_secs(1));
                }
            });
            let r = self.put_file(logical, ext, local);
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            said.thread().unpark();
            r
        })
        .inspect(|_| on(total, total))
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
        let _p = crate::timings::phase("records saved", crate::timings::Class::NasWrite);
        if let Some(dir) = &self.handoff {
            if self.changes.is_empty() && self.pending.is_empty() && self.checked.is_empty() {
                return Ok(());
            }
            let h = crate::handoff::Handoff { changes: self.changes.clone(), pending: self.pending.clone(), checked: self.checked.iter().cloned().collect(), done: None, raw: Vec::new() };
            crate::handoff::write(dir, &h)?;
            self.changes.clear();
            self.pending.clear();
            self.checked.clear();
            return Ok(());
        }
        check_writer(&self.root)?;
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

    /// `save`, under this Mac's build lock, held. (A record its changes leave as it is isn't
    /// written again: a step's last save often has none, and each write is a rename over a file
    /// another Mac may have open.)
    pub fn save_held(&mut self, _lock: &BuildLock) -> Result<()> {
        let dir = self.manifest_path.parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        let read: BTreeMap<String, String> = read_record(&self.manifest_path)?;
        let mut on_disk = read.clone();
        for (k, v) in &self.changes {
            match v {
                Some(n) => on_disk.insert(k.clone(), n.clone()),
                None => on_disk.remove(k),
            };
        }
        let pending_path = dir.join("pending.json");
        let pending_read: BTreeMap<String, String> = read_record(&pending_path)?;
        let mut pending = pending_read.clone();
        pending.extend(self.pending.clone());
        pending.retain(|k, _| !self.checked.contains(k));
        for (p, v, was) in [(&self.manifest_path, &on_disk, &read), (&pending_path, &pending, &pending_read)] {
            if v != was || !p.exists() {
                crate::whole::write(p, &serde_json::to_vec_pretty(v)?)?;
            }
        }
        // (What it reads next, a round's job's units still as they were.)
        self.manifest = match &self.as_of {
            Some(then) => units_as_of(&on_disk, then),
            None => on_disk,
        };
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

/// An error unless this Mac writes the build's records (docs/plan.md §8, Two Macs): the build Mac
/// alone does, so a write from another Mac (a step run by hand there, not as a helper's job) is
/// refused rather than let race it. Its agent's jobs are the build Mac's (SCENIC_BUILD_MAC); a step
/// run by hand is, on the Mac `state/build/writer` names (on any, before one is named). While the
/// pool is on (`state/pool/enabled`), none: every job hands off, and the lead's merge alone writes
/// them (docs/pool.md §7.3).
pub fn check_writer(root: &Path) -> Result<()> {
    // (The pool on, no one writes them but its lead's merge: a job hands off, docs/pool.md §7.3.)
    if root.join(crate::agent::pool::ENABLED).exists() {
        bail!("the pool is on ({}): the records are its lead's to write; a step saves only as a job's, handing off (SCENIC_HANDOFF)", root.join(crate::agent::pool::ENABLED).display());
    }
    if std::env::var_os("SCENIC_BUILD_MAC").is_some() {
        return Ok(());
    }
    let host = crate::agent::cond::host();
    match std::fs::read_to_string(root.join("state/build/writer")) {
        Ok(w) if w.trim() != host => bail!("{} is the build Mac, which alone writes the build's records; this Mac ({host}) saves only as a helper's job (SCENIC_HANDOFF)", w.trim()),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).context("read state/build/writer"),
    }
}

/// An exclusive lock for saving a root's manifest among this Mac's processes, in a local file named
/// after the root: released when dropped.
pub struct BuildLock(#[allow(dead_code)] std::fs::File);

impl BuildLock {
    fn file(root: &Path) -> Result<std::fs::File> {
        // By the root's real path: `x`, `x/` and a link to it are one build.
        let real = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let key = store::naming::hash16(real.to_string_lossy().as_bytes());
        let p = std::env::temp_dir().join(format!("scenic-build-{key}.lock"));
        std::fs::File::options().create(true).truncate(false).write(true).open(&p).with_context(|| format!("open {}", p.display()))
    }

    /// Waits until the lock is ours.
    pub fn take(root: &Path) -> Result<BuildLock> {
        let f = Self::file(root)?;
        crate::sys::lock(&f, true).context("lock the build manifest")?;
        Ok(BuildLock(f))
    }

    /// The lock when it's free now; None when another holds it (a paused job may, for hours).
    pub fn try_take(root: &Path) -> Result<Option<BuildLock>> {
        let f = Self::file(root)?;
        if !crate::sys::lock(&f, false).context("lock the build manifest")? {
            return Ok(None);
        }
        Ok(Some(BuildLock(f)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_root_and_its_spelling_with_a_slash_share_the_lock() {
        let d = tempfile::tempdir().unwrap();
        let held = BuildLock::take(d.path()).unwrap();
        let slashed = PathBuf::from(format!("{}/", d.path().display()));
        assert!(BuildLock::try_take(&slashed).unwrap().is_none());
        drop(held);
        // (A process another test is starting can hold a copy of the descriptor, and with it the
        // lock, between its fork and its exec: a few milliseconds, longer on a loaded Mac. A minute
        // at most.)
        let t = std::time::Instant::now();
        while BuildLock::try_take(&slashed).unwrap().is_none() {
            assert!(t.elapsed() < std::time::Duration::from_secs(60), "the lock wasn't free after it was dropped");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

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
    fn a_save_with_nothing_new_writes_nothing_and_still_reads_what_others_saved() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("root");
        let mut out = Out::open(&root, &d.path().join("s")).unwrap();
        out.changes.insert("a".into(), Some("a.1111111111111111.x".into()));
        out.save().unwrap();
        let m = root.join("state/build/manifest.json");
        let written = std::fs::metadata(&m).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        // Nothing new: the records stay as they were (no rename over a file another Mac may have
        // open), and what another step saved meanwhile is read.
        let mut other = Out::open(&root, &d.path().join("s2")).unwrap();
        other.changes.insert("b".into(), Some("b.2222222222222222.x".into()));
        other.save().unwrap();
        let theirs = std::fs::metadata(&m).unwrap().modified().unwrap();
        assert!(theirs > written);
        out.save().unwrap();
        assert_eq!(std::fs::metadata(&m).unwrap().modified().unwrap(), theirs);
        assert_eq!(out.get("b"), Some("b.2222222222222222.x"));
        // No temporary file left beside them.
        assert!(std::fs::read_dir(root.join("state/build")).unwrap().all(|e| !crate::whole::is_tmp(&e.unwrap().path())));
    }

    #[test]
    fn a_rounds_job_reads_the_units_as_they_were_and_saves_only_its_own_changes() {
        let then: BTreeMap<String, String> = [("base/6-1-1", "base/6-1-1.1111111111111111.base"), ("global/roads/6-1-1", "global/roads/6-1-1.1111111111111111.roads"), ("base/6-3-3", "base/6-3-3.3333333333333333.base")].into_iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        // Since: 6/1/1 rebuilt, 6/2/2 built, 6/3/3 pruned; a tile drawn.
        let now: BTreeMap<String, String> = [
            ("base/6-1-1", "base/6-1-1.aaaaaaaaaaaaaaaa.base"),
            ("global/roads/6-1-1", "global/roads/6-1-1.aaaaaaaaaaaaaaaa.roads"),
            ("base/6-2-2", "base/6-2-2.2222222222222222.base"),
            ("global/roadunits", "global/roadunits.4444444444444444.sect"),
            ("hidata/6-1-1", "hidata/6-1-1.5555555555555555.hidata"),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        let seen = units_as_of(&now, &then);
        assert_eq!(seen.get("base/6-1-1").map(String::as_str), Some("base/6-1-1.1111111111111111.base"));
        assert_eq!(seen.get("global/roads/6-1-1").map(String::as_str), Some("global/roads/6-1-1.1111111111111111.roads"));
        assert!(!seen.contains_key("base/6-2-2") && !seen.contains_key("base/6-3-3"));
        assert_eq!(seen.get("global/roadunits"), now.get("global/roadunits"));
        assert_eq!(seen.get("hidata/6-1-1"), now.get("hidata/6-1-1"));
        // A job opened with the round's file reads them so, and its save leaves the units in the
        // manifest as they are now.
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("root");
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        std::fs::write(root.join("state/build/manifest.json"), serde_json::to_vec(&now).unwrap()).unwrap();
        let round = d.path().join("round.json");
        std::fs::write(&round, serde_json::to_vec(&serde_json::json!({ "began": 1, "regions": ["a"], "last": false, "units": then })).unwrap()).unwrap();
        let spec = format!("{}#1", round.display());
        let mut out = Out::open_as_of(&root, &d.path().join("s"), Some(&spec)).unwrap();
        assert_eq!(out.manifest, seen);
        out.changes.insert("hidata/6-1-1".into(), Some("hidata/6-1-1.6666666666666666.hidata".into()));
        out.save().unwrap();
        assert_eq!(out.get("base/6-1-1"), Some("base/6-1-1.1111111111111111.base"));
        let on_disk: BTreeMap<String, String> = read_record(&root.join("state/build/manifest.json")).unwrap();
        let mut want = now.clone();
        want.insert("hidata/6-1-1".into(), "hidata/6-1-1.6666666666666666.hidata".into());
        assert_eq!(on_disk, want);
        // Its file unreadable, another round's, or the round over: the job fails rather than read
        // other units than its round's.
        assert!(Out::open_as_of(&root, &d.path().join("s"), Some(&format!("{}#1", d.path().join("none.json").display()))).is_err());
        assert!(Out::open_as_of(&root, &d.path().join("s"), Some(&format!("{}#2", round.display()))).is_err());
        std::fs::write(&round, serde_json::to_vec(&serde_json::json!({ "began": 1, "regions": ["a"], "last": false, "units": {}, "over": true })).unwrap()).unwrap();
        assert!(Out::open_as_of(&root, &d.path().join("s"), Some(&spec)).is_err());
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
        // The pool on: no one's step writes them, the build Mac's job's neither (it hands off).
        std::fs::create_dir_all(root.join("state/pool")).unwrap();
        std::fs::write(root.join(crate::agent::pool::ENABLED), "").unwrap();
        out.changes.insert("b".into(), Some("b.2222222222222222.x".into()));
        assert!(format!("{:#}", out.save().unwrap_err()).contains("the pool is on"));
        assert!(check_writer(&root).is_err());
    }
}
