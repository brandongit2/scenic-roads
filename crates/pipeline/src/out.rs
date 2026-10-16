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
    /// Logical name → content name of every file this build wrote or reused (a map update's job: with
    /// the units as they were when the map update began, `as_of`).
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
    /// A map update's job ($SCENIC_UNITS_AS_OF): the units' outputs as they were when the map update began,
    /// read in place of the manifest's own (`units_as_of`).
    as_of: Option<BTreeMap<String, String>>,
    /// The step whose job this is (`for_step`): it puts and removes only the names its outputs
    /// function gives (crate::agent::steps, the one the lead's merge checks every entry against),
    /// and a name outside them fails the job. None: no step (the agent merging hand-offs, a tool).
    step: Option<String>,
    /// Names set or removed outside the step's outputs: the save fails, naming the first.
    outside: Vec<String>,
}

/// A unit's outputs that map tiles, the road index and rail stops read: its base pack, road values
/// and roads' English (a prune of a unit drops them; its job also writes analysis grids, which
/// nothing a map update makes reads).
pub const UNIT_OUTPUTS: [&str; 3] = ["base/", "global/roads/", "global/roaden/"];

/// What a map update fixes as it begins (`units_as_of`): the units' outputs, and the 3D buildings' hi
/// packs, so a bldtiles job ending meanwhile doesn't change the map update's catalog (made again and
/// again as their jobs end); they go out with the next.
pub const AS_OF_OUTPUTS: [&str; 4] = ["base/", "global/roads/", "global/roaden/", "layers/buildings/hi/"];

/// Names the file of a map update under way and when it began, `<path>#<began>` (agent::build::MapUpdate, in
/// the agent's folder): a map update's jobs (its map tiles, road index, rail stops and catalog) read the
/// units as they were when it began (`units_as_of`), so what's built meanwhile changes nothing the
/// map update makes; it waits for the next.
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

/// The map update's file in the agent's folder (crate::agent).
pub const MAP_UPDATE_FILE: &str = "map-update.json";

/// The units of a map update's file (`UNITS_AS_OF_ENV`: `<path>#<began>`): an error when it can't be
/// read, or it's another map update's, or the map update's over, so a map update's job fails rather
/// than make its part from other units than the map update's.
fn read_as_of(spec: &str) -> Result<BTreeMap<String, String>> {
    #[derive(serde::Deserialize)]
    struct MapUpdate {
        began: u64,
        units: BTreeMap<String, String>,
        #[serde(default)]
        over: bool,
    }
    let (p, began) = spec.rsplit_once('#').and_then(|(p, b)| Some((p, b.parse::<u64>().ok()?))).with_context(|| format!("{UNITS_AS_OF_ENV}={spec:?}: not <path>#<began>"))?;
    let b = std::fs::read(p).with_context(|| format!("read the map update's units, {p}"))?;
    let r: MapUpdate = serde_json::from_slice(&b).with_context(|| format!("parse {p}"))?;
    if r.began == began && !r.over {
        return Ok(r.units);
    }
    anyhow::bail!("{p} is no longer the map update this job is for (it began at {began}; the file's began at {}{})", r.began, if r.over { ", and is over" } else { "" })
}

/// Hand-off `h`'s changes laid over manifest `m` (a name it removes, removed).
pub fn lay_over(m: &mut BTreeMap<String, String>, h: &crate::handoff::Handoff) {
    for (l, c) in &h.changes {
        match c {
            Some(c) => m.insert(l.clone(), c.clone()),
            None => m.remove(l),
        };
    }
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

/// A file's `hash16` (BLAKE3, its content name's) and SHA-256 (the NAS's check, `verify`), from one
/// read of it: each chunk hashed both ways at once, on two threads.
fn hashes_of(p: &Path) -> Result<(String, String)> {
    let mut f = std::fs::File::open(p)?;
    let mut b3 = blake3::Hasher::new();
    let mut sha = sha2::Sha256::new();
    let mut buf = vec![0u8; 8 << 20];
    loop {
        let mut n = 0;
        while n < buf.len() {
            match f.read(&mut buf[n..])? {
                0 => break,
                k => n += k,
            }
        }
        if n == 0 {
            break;
        }
        let chunk = &buf[..n];
        // (In turn where there are no threads: the WebAssembly builds.)
        let threaded = std::thread::scope(|s| {
            let other = std::thread::Builder::new().spawn_scoped(s, || sha.update(chunk));
            b3.update(chunk);
            other.is_ok()
        });
        if !threaded {
            sha.update(chunk);
        }
        store::naming::count_moved(n);
    }
    Ok((store::naming::hex16(&b3.finalize()), format!("{:x}", sha.finalize())))
}


impl Out {
    /// A job of `step`'s: `root`, the NAS project folder (or a local folder standing in for it);
    /// `scratch`, local space. It puts and removes only `step`'s outputs (`for_step`).
    pub fn open(root: &Path, scratch: &Path, step: &str) -> Result<Self> {
        let mut out = Self::open_as_of(root, scratch, std::env::var(UNITS_AS_OF_ENV).ok().as_deref())?;
        out.for_step(step);
        Ok(out)
    }

    /// Not a step's job's: the agent merging hand-offs, a tool, a test of a step's parts. It may
    /// put and remove any name, so a step's job opens with `open`, never this.
    pub fn unbound(root: &Path, scratch: &Path) -> Result<Self> {
        Self::open_as_of(root, scratch, std::env::var(UNITS_AS_OF_ENV).ok().as_deref())
    }

    /// `open`, a map update's job's (`as_of`: its map update's file and when it began, `UNITS_AS_OF_ENV`).
    pub fn open_as_of(root: &Path, scratch: &Path, as_of: Option<&str>) -> Result<Self> {
        let unmerged = std::env::var_os(crate::agent::resume::UNMERGED_ENV).map(PathBuf::from);
        Self::open_with(root, scratch, as_of, unmerged.as_deref())
    }

    /// `open_as_of`, with this Mac's hand-offs for the job's step its lead hasn't merged
    /// (`unmerged`: a file of one hand-off, their changes laid in order, made by its agent with
    /// `state/pool/resume` on: crate::agent::resume) laid over the snapshot, as
    /// `Keys::load_with` lays the done records: a job resuming its own work (the OSM pass after
    /// sleep) finds its earlier stages' outputs (docs/pool.md §7.4). Its agent gives them only to a
    /// job that hands off (`SCENIC_HANDOFF`): one that saves the records has none. They're the snapshot's,
    /// not this job's changes: its saves hand off only its own.
    pub fn open_with(root: &Path, scratch: &Path, as_of: Option<&str>, unmerged: Option<&Path>) -> Result<Self> {
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
        let mut manifest = manifest;
        if let Some(p) = unmerged {
            let b = std::fs::read(p).with_context(|| format!("read this Mac's unmerged hand-offs, {}", p.display()))?;
            let h: crate::handoff::Handoff = serde_json::from_slice(&b).with_context(|| format!("parse {}", p.display()))?;
            lay_over(&mut manifest, &h);
        }
        // (A helper's job hands off only its own uploads.)
        let pending = if handoff.is_some() { BTreeMap::new() } else { read_record(&root.join("state/build/pending.json"))? };
        Ok(Out { root: root.to_path_buf(), manifest, changes: BTreeMap::new(), checked: Default::default(), pending, manifest_path, scratch: scratch.to_path_buf(), handoff, as_of, step: None, outside: Vec::new() })
    }

    /// Binds this to a job of `step` (scenic-build, as it starts a step): from now on it puts and
    /// removes only `step`'s outputs (crate::agent::steps::writes, the function the lead's merge
    /// checks every entry against, so the writer and the check can't disagree; docs/pool.md §7.3).
    /// A put outside them fails before anything is uploaded; a `set` or `remove` outside them fails
    /// the next save, and nothing of that save lands (what earlier saves saved, the targets done
    /// before it, stands). A step the steps table doesn't know (a tool run by
    /// hand) is held to nothing while it writes the manifest, and changes nothing when it hands off
    /// (the merge refuses such an entry).
    fn for_step(&mut self, step: &str) {
        self.step = Some(step.to_string());
    }

    /// Whether this job may put (`removal`: remove) `logical`; why not.
    fn admits(&self, logical: &str, removal: bool) -> Result<()> {
        let Some(step) = &self.step else { return Ok(()) };
        match crate::agent::steps::writes(step, logical, removal) {
            Some(true) => Ok(()),
            Some(false) => bail!("{} {logical}, outside {step}'s outputs (crate::agent::steps)", if removal { "removes" } else { "writes" }),
            None if self.handoff.is_some() => bail!("{} {logical}: {step} isn't a step the steps table knows, so its hand-off may change nothing", if removal { "removes" } else { "writes" }),
            None => Ok(()),
        }
    }

    /// Whether this job may remove `logical` (its step's outputs function allows it): a step's
    /// sweep of its stale names removes only those, so a name its outputs don't give (a legacy or
    /// malformed one under its prefix) stays, rather than fail its every save.
    pub fn may_remove(&self, logical: &str) -> bool {
        self.admits(logical, true).is_ok()
    }

    /// `admits`, for a `set` or `remove`, which can't fail: one outside is kept for the save.
    fn note(&mut self, logical: &str, removal: bool) {
        if let Err(e) = self.admits(logical, removal) {
            self.outside.push(format!("{e:#}"));
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The NAS path of a content name. A path in a drop box (`inputs/`, `translations/`,
    /// `descriptions/`) is refused, naming the caller: a step reads an input's accepted version
    /// through `crate::inputs::open` alone (docs/inputs.md §7.1), so a reader missed fails its
    /// first run instead of bypassing the gate.
    #[track_caller]
    pub fn path(&self, content: &str) -> PathBuf {
        if crate::inputs::in_drop_box(content) {
            panic!("{}: {content} is in a drop box; a step reads an input through crate::inputs::open alone (docs/inputs.md §7.1)", std::panic::Location::caller());
        }
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
        self.admits(logical, false)?;
        // (Its name's hash and its checksum from one read of it: `write_atomic_hashed` checks the
        // read-back against the hash, as against its own.)
        let size = std::fs::metadata(local)?.len();
        let (h, sha) = hashes_of(local)?;
        let name = store::naming::content_name(logical, &h, ext);
        let dest = self.root.join(&name);
        let mut existed = dest.exists();
        let got = store::naming::write_atomic_hashed(&self.root, logical, ext, local, &h, size)?;
        anyhow::ensure!(got == name, "{logical}: wrote {got}, expected {name}");
        if existed {
            // In use again: a fresh time keeps GC's age rule from taking it before a catalog does
            // (GC looks at a file's time again right before it removes it). One that can't be
            // touched, or is gone once touched (GC took it between), is uploaded again.
            let touched = std::fs::File::options().write(true).open(&dest).and_then(|f| f.set_modified(std::time::SystemTime::now())).is_ok();
            if !touched || !dest.exists() {
                existed = false;
                let again = store::naming::write_atomic_hashed(&self.root, logical, ext, local, &h, size)?;
                anyhow::ensure!(again == name, "{logical}: wrote {again}, expected {name}");
            }
        }
        let sha = (!existed).then_some(sha);
        // (Its bytes, to the phase it's uploaded in: crate::timings.)
        crate::timings::count(if existed { 0 } else { std::fs::metadata(local).map(|m| m.len()).unwrap_or(0) }, 1);
        if let Some(sha) = sha {
            self.pending.insert(name.clone(), sha);
        }
        std::fs::remove_file(local).ok();
        self.manifest.insert(logical.to_string(), name.clone());
        self.changes.insert(logical.to_string(), Some(name.clone()));
        Ok(name)
    }

    /// `put_file`, saying how far it is to `on` about once a second, as (bytes, total): it reads the
    /// file twice (its name's hash and its checksum; the copy) and the copy once more on the NAS, for
    /// a large file most of a step's time.
    pub fn put_file_with(&mut self, logical: &str, ext: &str, local: &Path, on: &(dyn Fn(u64, u64) + Sync)) -> Result<String> {
        let total = 3 * std::fs::metadata(local)?.len();
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

    /// Uploads bytes as `<logical>.<hash16>.<ext>` without naming them in the manifest (the gate's
    /// checked copies, which its index names: crate::inputs::gate), and returns the content name.
    /// One there already is reused and touched, as `put_file` does; one that can't be touched is
    /// taken as an upload of its own, its check pending, and one gone once touched (GC took it
    /// between: its tombstone sweep) is uploaded again.
    pub fn store_bytes(&mut self, logical: &str, ext: &str, bytes: &[u8]) -> Result<String> {
        let h = store::naming::hash16(bytes);
        let name = store::naming::content_name(logical, &h, ext);
        let dest = self.root.join(&name);
        let mut existed = dest.exists();
        let local = self.scratch_file(&format!("{logical}.{ext}"));
        std::fs::write(&local, bytes)?;
        let got = store::naming::write_atomic_hashed(&self.root, logical, ext, &local, &h, bytes.len() as u64);
        if got.is_ok() && existed {
            let touched = std::fs::File::options().write(true).open(&dest).and_then(|f| f.set_modified(std::time::SystemTime::now())).is_ok();
            if !touched || !dest.exists() {
                existed = false;
                let again = store::naming::write_atomic_hashed(&self.root, logical, ext, &local, &h, bytes.len() as u64);
                std::fs::remove_file(&local).ok();
                anyhow::ensure!(again? == name, "{logical}: wrote another name than {name}");
            }
        }
        std::fs::remove_file(&local).ok();
        anyhow::ensure!(got? == name, "{logical}: wrote another name than {name}");
        crate::timings::count(if existed { 0 } else { bytes.len() as u64 }, 1);
        if !existed {
            self.pending.insert(name.clone(), format!("{:x}", sha2::Sha256::digest(bytes)));
        }
        Ok(name)
    }

    /// Names `content` (None: nothing) under `logical` in the manifest: a file uploaded already
    /// (`store_bytes`).
    pub fn set(&mut self, logical: &str, content: Option<String>) {
        self.note(logical, content.is_none());
        match &content {
            Some(c) => self.manifest.insert(logical.to_string(), c.clone()),
            None => self.manifest.remove(logical),
        };
        self.changes.insert(logical.to_string(), content);
    }

    /// Upload bytes under `logical`.
    pub fn put_bytes(&mut self, logical: &str, ext: &str, bytes: &[u8]) -> Result<String> {
        self.admits(logical, false)?;
        let local = self.scratch_file(&format!("{logical}.{ext}"));
        std::fs::write(&local, bytes)?;
        self.put_file(logical, ext, &local)
    }

    /// Record a logical name as gone (its file stays until GC).
    pub fn remove(&mut self, logical: &str) {
        self.note(logical, true);
        self.manifest.remove(logical);
        self.changes.insert(logical.to_string(), None);
    }

    /// Writes this run's changes into the manifest on disk (re-read under a lock, so another step's
    /// changes saved meanwhile are kept), and the unverified uploads likewise. A helper's job hands
    /// them off instead (crate::handoff): in the pool, every job's (`check_writer`).
    pub fn save(&mut self) -> Result<()> {
        let _p = crate::timings::phase("records saved", crate::timings::Class::NasWrite);
        // (Nothing saved of a job that named something outside its step's outputs.)
        if let Some(first) = self.outside.first() {
            bail!("{first}{}: nothing saved", if self.outside.len() > 1 { format!(" (and {} more)", self.outside.len() - 1) } else { String::new() });
        }
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
        // (What it reads next, a map update's job's units still as they were.)
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
            let mut cmd = store::spawn::command(ssh[0]);
            cmd.args(&ssh[1..]);
            let quoted: Vec<String> = chunk.iter().map(|n| format!("'{}'", n.replace('\'', "'\\''"))).collect();
            cmd.arg(format!("cd '{nas_root}' && sha256sum {}", quoted.join(" ")));
            let out = store::spawn::output(&mut cmd).context("ssh to the NAS")?;
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
            let mut cmd = store::spawn::command(ssh[0]);
            cmd.args(&ssh[1..]);
            cmd.arg(format!("cd '{nas_root}' && rm -f {}", quoted.join(" ")));
            store::spawn::status(&mut cmd).ok();
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

/// An error once the pool has begun (its first term made): the records are its lead's to write,
/// through its merge of the journal alone, and every job hands its saves off (SCENIC_HANDOFF,
/// docs/pool.md §7.3), whatever Mac it runs on. A folder no term was made in yet (a test's, a
/// scratch build's) has its records written as today's files (term 1's first snapshot is made from
/// them).
pub fn check_writer(root: &Path) -> Result<()> {
    let terms = root.join(crate::pool::term::DIR);
    if terms.exists() {
        bail!("the pool leads this build ({} exists): the records are its lead's to write; a step saves only as a job's, handing off (SCENIC_HANDOFF)", terms.display());
    }
    Ok(())
}

/// An exclusive lock for saving a root's manifest among this Mac's processes, in a local file named
/// after the root: released when dropped.
pub struct BuildLock(#[allow(dead_code)] crate::sys::PathLock);

impl BuildLock {
    fn file(root: &Path) -> PathBuf {
        // By the root's real path: `x`, `x/` and a link to it are one build.
        let real = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let key = store::naming::hash16(real.to_string_lossy().as_bytes());
        std::env::temp_dir().join(format!("scenic-build-{key}.lock"))
    }

    /// Waits until the lock is ours.
    pub fn take(root: &Path) -> Result<BuildLock> {
        let l = crate::sys::PathLock::take(&Self::file(root), true).context("lock the build manifest")?;
        Ok(BuildLock(l.context("lock the build manifest: not taken")?))
    }

    /// The lock when it's free now; None when another holds it (a paused job may, for hours).
    pub fn try_take(root: &Path) -> Result<Option<BuildLock>> {
        Ok(crate::sys::PathLock::take(&Self::file(root), false).context("lock the build manifest")?.map(BuildLock))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This Mac's unmerged hand-offs laid over the snapshot (docs/pool.md §7.4): their names read
    /// as the job's earlier attempts left them, none of them this job's own changes; none given,
    /// the snapshot as it is.
    #[test]
    fn a_jobs_unmerged_hand_offs_are_laid_over_the_snapshot() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        std::fs::write(root.join("state/build/manifest.json"), r#"{"sources/osm/2026-09-01/pass": "sources/osm/2026-09-01/pass.1111111111111111.json", "sources/osm/2026-10-01/sets/old": "x"}"#).unwrap();
        let h = crate::handoff::Handoff { changes: [("sources/osm/2026-10-01/filtered".to_string(), Some("sources/osm/2026-10-01/filtered.2222222222222222.osm.pbf".to_string())), ("sources/osm/2026-10-01/sets/old".to_string(), None)].into(), ..Default::default() };
        let f = d.path().join("unmerged.json");
        std::fs::write(&f, serde_json::to_vec(&h).unwrap()).unwrap();
        let mut out = Out::open_with(&root, &d.path().join("scratch"), None, Some(&f)).unwrap();
        out.for_step("osm-pass");
        assert_eq!(out.get("sources/osm/2026-10-01/filtered"), Some("sources/osm/2026-10-01/filtered.2222222222222222.osm.pbf"));
        assert!(out.get("sources/osm/2026-10-01/sets/old").is_none() && out.get("sources/osm/2026-09-01/pass").is_some());
        assert!(out.changes.is_empty(), "the snapshot's, not this job's");
        let plain = Out::open_with(&root, &d.path().join("scratch"), None, None).unwrap();
        assert!(plain.get("sources/osm/2026-10-01/filtered").is_none() && plain.get("sources/osm/2026-10-01/sets/old").is_some());
        // (A file that can't be read fails the job rather than open the snapshot without them.)
        assert!(Out::open_with(&root, &d.path().join("scratch"), None, Some(&d.path().join("missing.json"))).is_err());
    }

    /// One read gives the name's hash and the checksum, each as hashing the bytes alone gives it,
    /// across chunks (a file larger than one, its last one short).
    #[test]
    fn a_files_two_hashes_from_one_read() {
        let d = tempfile::tempdir().unwrap();
        for len in [0usize, 5, (8 << 20) + 12345] {
            let p = d.path().join(format!("f{len}"));
            let bytes: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
            std::fs::write(&p, &bytes).unwrap();
            let (h, sha) = hashes_of(&p).unwrap();
            assert_eq!(h, store::naming::hash16(&bytes));
            assert_eq!(sha, format!("{:x}", sha2::Sha256::digest(&bytes)));
        }
    }

    /// A file reused by name is touched (GC's age rule, looked at again right before it removes a
    /// file, then passes it over); one that can't be touched is uploaded as new (pending its check).
    #[test]
    fn a_file_reused_by_name_is_touched_or_uploaded_again() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let mut out = Out::unbound(&root, &d.path().join("scratch")).unwrap();
        let local = |n: &str| {
            let p = d.path().join(n);
            std::fs::write(&p, b"the same bytes").unwrap();
            p
        };
        let name = out.put_file("work/x/6-1-1", "bin", &local("a")).unwrap();
        out.pending.clear();
        let dest = root.join(&name);
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86400);
        std::fs::File::options().write(true).open(&dest).unwrap().set_modified(old).unwrap();
        assert_eq!(out.put_file("work/x/6-1-1", "bin", &local("b")).unwrap(), name);
        let t = std::fs::metadata(&dest).unwrap().modified().unwrap();
        assert!(t.elapsed().unwrap() < std::time::Duration::from_secs(600), "touched");
        assert!(out.pending.is_empty(), "reused, not uploaded");
        // Can't be touched (read-only here): taken as an upload of its own, its check pending.
        let mut perm = std::fs::metadata(&dest).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(&dest, perm).unwrap();
        assert_eq!(out.put_file("work/x/6-1-1", "bin", &local("c")).unwrap(), name);
        assert!(out.pending.contains_key(&name));
    }

    /// The gate's copies (`store_bytes`) likewise: one reused is touched; one that can't be touched
    /// is taken as an upload of its own, its check pending; one GC took between (gone once touched:
    /// its tombstone sweep) is uploaded again, so the index the gate writes never names a file that
    /// isn't there.
    #[test]
    fn a_stored_copy_reused_is_touched_or_uploaded_again() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let mut out = Out::unbound(&root, &d.path().join("scratch")).unwrap();
        let name = out.store_bytes("sources/inputs/_gate-test/a", "jsonl", b"the same lines").unwrap();
        out.pending.clear();
        let dest = root.join(&name);
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86400);
        std::fs::File::options().write(true).open(&dest).unwrap().set_modified(old).unwrap();
        assert_eq!(out.store_bytes("sources/inputs/_gate-test/a", "jsonl", b"the same lines").unwrap(), name);
        assert!(std::fs::metadata(&dest).unwrap().modified().unwrap().elapsed().unwrap() < std::time::Duration::from_secs(600), "touched");
        assert!(out.pending.is_empty(), "reused, not uploaded");
        // Can't be touched (read-only here): taken as an upload of its own, its check pending.
        let mut perm = std::fs::metadata(&dest).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(&dest, perm).unwrap();
        assert_eq!(out.store_bytes("sources/inputs/_gate-test/a", "jsonl", b"the same lines").unwrap(), name);
        assert!(out.pending.contains_key(&name));
        // Gone (GC took it): written again, there.
        let mut perm = std::fs::metadata(&dest).unwrap().permissions();
        perm.set_readonly(false);
        std::fs::set_permissions(&dest, perm).unwrap();
        std::fs::remove_file(&dest).unwrap();
        out.pending.clear();
        assert_eq!(out.store_bytes("sources/inputs/_gate-test/a", "jsonl", b"the same lines").unwrap(), name);
        assert!(dest.exists() && out.pending.contains_key(&name));
    }

    #[test]
    fn a_root_and_its_spelling_with_a_slash_share_the_lock() {
        let d = tempfile::tempdir().unwrap();
        let held = BuildLock::take(d.path()).unwrap();
        let slashed = PathBuf::from(format!("{}/", d.path().display()));
        assert!(BuildLock::try_take(&slashed).unwrap().is_none());
        drop(held);
        // (Free at once, however loaded the Mac: its file went with it, so a copy of its descriptor
        // a process another test is starting holds between its fork and its exec holds nothing:
        // crate::sys::PathLock.)
        assert!(BuildLock::try_take(&slashed).unwrap().is_some(), "the lock wasn't free after it was dropped");
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
        let mut out = Out::unbound(&root, &d.path().join("s")).unwrap();
        out.changes.insert("a".into(), Some("a.1111111111111111.x".into()));
        std::fs::create_dir_all(root.join("state/build/manifest.json")).unwrap();
        assert!(out.save().is_err());
        assert!(root.join("state/build/manifest.json").is_dir());
        // And opening with it unreadable fails too.
        assert!(Out::unbound(&root, &d.path().join("s")).is_err());
    }

    #[test]
    fn a_save_with_nothing_new_writes_nothing_and_still_reads_what_others_saved() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("root");
        let mut out = Out::unbound(&root, &d.path().join("s")).unwrap();
        out.changes.insert("a".into(), Some("a.1111111111111111.x".into()));
        out.save().unwrap();
        let m = root.join("state/build/manifest.json");
        let written = std::fs::metadata(&m).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        // Nothing new: the records stay as they were (no rename over a file another Mac may have
        // open), and what another step saved meanwhile is read.
        let mut other = Out::unbound(&root, &d.path().join("s2")).unwrap();
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
    fn a_map_updates_job_reads_the_units_as_they_were_and_saves_only_its_own_changes() {
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
        // A job opened with the map update's file reads them so, and its save leaves the units in the
        // manifest as they are now.
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("root");
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        std::fs::write(root.join("state/build/manifest.json"), serde_json::to_vec(&now).unwrap()).unwrap();
        let map_update = d.path().join("map-update.json");
        std::fs::write(&map_update, serde_json::to_vec(&serde_json::json!({ "began": 1, "regions": ["a"], "last": false, "units": then })).unwrap()).unwrap();
        let spec = format!("{}#1", map_update.display());
        let mut out = Out::open_as_of(&root, &d.path().join("s"), Some(&spec)).unwrap();
        assert_eq!(out.manifest, seen);
        out.changes.insert("hidata/6-1-1".into(), Some("hidata/6-1-1.6666666666666666.hidata".into()));
        out.save().unwrap();
        assert_eq!(out.get("base/6-1-1"), Some("base/6-1-1.1111111111111111.base"));
        let on_disk: BTreeMap<String, String> = read_record(&root.join("state/build/manifest.json")).unwrap();
        let mut want = now.clone();
        want.insert("hidata/6-1-1".into(), "hidata/6-1-1.6666666666666666.hidata".into());
        assert_eq!(on_disk, want);
        // Its file unreadable, another map update's, or the map update over: the job fails rather than read
        // other units than its map update's.
        assert!(Out::open_as_of(&root, &d.path().join("s"), Some(&format!("{}#1", d.path().join("none.json").display()))).is_err());
        assert!(Out::open_as_of(&root, &d.path().join("s"), Some(&format!("{}#2", map_update.display()))).is_err());
        let e = read_as_of(&format!("{}#2", map_update.display())).unwrap_err().to_string();
        assert!(e.contains("no longer the map update") && e.contains("began at 1"), "{e}");
        std::fs::write(&map_update, serde_json::to_vec(&serde_json::json!({ "began": 1, "regions": ["a"], "last": false, "units": {}, "over": true })).unwrap()).unwrap();
        assert!(Out::open_as_of(&root, &d.path().join("s"), Some(&spec)).is_err());
    }

    #[test]
    fn once_the_pool_has_begun_no_step_saves_the_records() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("root");
        let mut out = Out::unbound(&root, &d.path().join("s")).unwrap();
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        // No term made: saved, as today's files.
        out.changes.insert("a".into(), Some("a.1111111111111111.x".into()));
        out.save().unwrap();
        assert!(root.join("state/build/manifest.json").exists());
        // The pool begun (a term made): no one's step writes them (a job hands off).
        std::fs::create_dir_all(root.join(crate::pool::term::DIR)).unwrap();
        out.changes.insert("b".into(), Some("b.2222222222222222.x".into()));
        assert!(format!("{:#}", out.save().unwrap_err()).contains("the pool leads this build"));
        assert!(check_writer(&root).is_err());
    }

    /// A job puts and removes only its step's outputs (crate::agent::steps::writes, the function
    /// the lead's merge checks by): a put outside fails before it uploads, a set or removal outside
    /// fails the save before anything is saved, and a step the table doesn't know hands off nothing.
    #[test]
    fn a_job_writes_only_its_steps_outputs() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("root");
        let local = d.path().join("f.bin");
        let mut out = Out::open(&root, &d.path().join("s"), "pack").unwrap();
        // Its own: a map tile's data and road pack, a rail pack removed.
        std::fs::write(&local, b"tile").unwrap();
        out.put_file("hidata/6-1-2", "hidata", &local).unwrap();
        out.put_bytes("layers/roads/hi/6-1-2", "pack", b"roads").unwrap();
        out.remove("layers/rails/hi/6-1-2");
        out.save().unwrap();
        let saved: BTreeMap<String, String> = read_record(&root.join("state/build/manifest.json")).unwrap();
        assert_eq!(saved.keys().collect::<Vec<_>>(), ["hidata/6-1-2", "layers/roads/hi/6-1-2"]);
        // Another step's file: refused before it's uploaded, the local file left.
        std::fs::write(&local, b"terrain").unwrap();
        let e = format!("{:#}", out.put_file("layers/terrain/hi/6-1-2", "pack", &local).unwrap_err());
        assert!(e.contains("writes layers/terrain/hi/6-1-2, outside pack's outputs"), "{e}");
        assert!(local.exists() && !root.join("layers/terrain").exists());
        assert!(out.put_bytes("layers/roads/hi/7-1-2", "pack", b"x").is_err(), "a name of the wrong zoom");
        // A set and a removal outside: the next save fails, saving nothing, whatever came with them.
        out.put_bytes("layers/roads/hi/6-1-3", "pack", b"more").unwrap();
        out.set("layers/terrain/hi/6-1-3", Some("layers/terrain/hi/6-1-3.1111111111111111.pack".into()));
        out.remove("base/6-1-3");
        let e = format!("{:#}", out.save().unwrap_err());
        assert!(e.contains("layers/terrain/hi/6-1-3") && e.contains("and 1 more") && e.contains("nothing saved"), "{e}");
        assert_eq!(read_record::<BTreeMap<String, String>>(&root.join("state/build/manifest.json")).unwrap(), saved);
        assert!(out.save().is_err(), "it stays failed");
        // A step the table doesn't know: held to nothing while it writes the manifest (a tool run
        // by hand) and to no change at all when it hands off (the merge refuses such an entry).
        let mut tool = Out::open(&root, &d.path().join("s2"), "patch-ferries").unwrap();
        tool.put_bytes("sources/osm/2026-09-28/pieces/6-1-2", "osm", b"p").unwrap();
        tool.handoff = Some(d.path().join("handoff"));
        assert!(format!("{:#}", tool.put_bytes("sources/osm/2026-09-28/pieces/6-1-3", "osm", b"q").unwrap_err()).contains("isn't a step the steps table knows"));
        // Not bound to a step (the agent merging hand-offs): anything.
        let mut merge = Out::unbound(&root, &d.path().join("s3")).unwrap();
        merge.set("layers/terrain/hi/6-1-4", Some("layers/terrain/hi/6-1-4.2222222222222222.pack".into()));
        merge.save().unwrap();
    }
}
