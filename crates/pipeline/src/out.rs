//! Where build steps put their outputs: the NAS project folder (docs/storage.md), as immutable
//! content-named files, with a build manifest (logical name → file) the catalogue is made from.
//!
//! A file is first written locally (this Mac's SSD), hashed, then copied to `<name>.tmp` on
//! the NAS, read back past the client's cache and checked against its hash, and renamed into place
//! (`store::naming::write_atomic`). A file already there is reused and touched, so GC sees it as
//! in use (one the manifest names under the same logical name with one look: `put_file`). An upload copied stays pending until `verify` checks it on the NAS itself (SHA-256 over SSH, GC's daily job).
//!
//! Several steps may run at once on the lead (the agent's job and a manual one): each records
//! its own changes and merges them into the manifest on disk under this Mac's lock (`BuildLock`)
//! when it saves. A helper's job on the other Mac (docs/building.md §2, The pool; `SCENIC_HANDOFF`)
//! saves its changes as hand-offs instead (crate::handoff), which the lead's agent merges: the
//! lead alone writes the manifest.

use anyhow::{bail, Context, Result};
use sha2::Digest;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use store::nasfs::NasPath;

pub struct Out {
    root: store::nasfs::NasRoot,
    /// Logical name → content name of every file this build wrote or reused.
    pub manifest: BTreeMap<String, String>,
    /// This run's changes to the manifest (None: removed), merged into the file when saving.
    changes: BTreeMap<String, Option<String>>,
    /// Uploads this run checked (no longer pending), likewise.
    checked: std::collections::BTreeSet<String>,
    /// Content name → SHA-256 (hex) of uploads not yet verified on the NAS.
    pending: BTreeMap<String, String>,
    /// Content name → size (bytes) of the files this run wrote or reused, handed on with the
    /// manifest changes that name them (`SIZES`, the catalogues' sizes without a look at the NAS).
    sizes: BTreeMap<String, u64>,
    manifest_path: NasPath,
    /// Local scratch space for files before upload.
    pub scratch: PathBuf,
    /// A helper's job: where its saves go as hand-offs ($SCENIC_HANDOFF), never the manifest.
    handoff: Option<PathBuf>,
    /// The step whose job this is (`for_step`): it puts and removes only the names its outputs
    /// function gives (crate::agent::steps, the one the lead's merge checks every entry against),
    /// and a name outside them fails the job. None: no step (the agent merging hand-offs, a tool).
    step: Option<String>,
    /// Names set or removed outside the step's outputs: the save fails, naming the first.
    outside: Vec<String>,
    /// Uploads put off (`put_file_later`), made in turn on a thread beside the job.
    later: Later,
    /// Logical name → content name of each file this run uploaded (or reused there by `upload`)
    /// and landed, until set or removed: one put again unchanged isn't looked at on the NAS again
    /// (`put_file`).
    uploaded: BTreeMap<String, String>,
}

/// The uploads put off (`Out::put_file_later`): sent to a thread of their own that makes them in
/// turn while the job goes on, their results back in the same order. Each is named in the manifest
/// only once it has landed (`Out::settle`), so a save never names a file that isn't on the NAS.
#[derive(Default)]
struct Later {
    to: Option<std::sync::mpsc::Sender<Upload>>,
    /// (In a mutex: an `Out` is shared by reference between threads.)
    back: Option<std::sync::Mutex<std::sync::mpsc::Receiver<Result<Landed>>>>,
    /// The uploads sent and those landed (taken back): `Mark`s count them.
    sent: u64,
    landed: u64,
    /// Each sent upload's size and logical name not yet landed, in order (its deadline; a name in
    /// flight, which `put_file`, `set` and `remove` settle first).
    sizes: std::collections::VecDeque<(u64, String)>,
    /// The bytes copied by uploads landed and not yet claimed (`claim_written`), by their order
    /// sent: the target that put them off claims them for its cost line.
    unclaimed: Vec<(u64, u64)>,
    /// The first upload that failed or didn't land: every later save fails with it.
    failed: Option<String>,
}

/// An upload put off.
struct Upload {
    root: store::nasfs::NasRoot,
    logical: String,
    ext: String,
    local: PathBuf,
}

/// An upload made (`upload`): where it went, and what the records need of it.
struct Landed {
    logical: String,
    name: String,
    /// Its SHA-256, when it was copied (not one already there): its check is pending.
    sha: Option<String>,
    size: u64,
    /// How long its copy took (ms; 0: none made).
    ms: u64,
}

/// The uploads put off up to a moment (`Out::mark`): `Out::save_upto` waits for those alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark(u64);

/// How long a put-off upload may take before the job fails: a minute, and a second for each
/// 2 MB (a file is read twice here and once on the NAS; a share answering at a few MB/s still
/// lands well inside it). An upload stuck in the kernel no longer holds the job forever: its
/// thread is left to it, and the job fails (its retry takes over).
pub fn upload_deadline(bytes: u64) -> std::time::Duration {
    std::time::Duration::from_secs(60 + (bytes >> 21))
}

/// The waits before a put-off upload that failed transiently is tried again (an SMB moment of
/// ENOENT, EIO or a reset connection): its file is content-named, so a retry lands the same file.
const UPLOAD_WAITS: &[std::time::Duration] = if cfg!(test) {
    &[std::time::Duration::from_millis(1)]
} else {
    &[std::time::Duration::from_secs(2), std::time::Duration::from_secs(5), std::time::Duration::from_secs(15)]
};

/// The records' sizes of the files the manifest names (content name → bytes, compact JSON), kept
/// beside the manifest by its writer (`Out::save_held`; in the pool its lead, from its records):
/// what a catalogue lists a file's size as without looking at the NAS. A file written before them
/// isn't in it (the catalogue looks at it once, then has it from the catalogue before).
pub const SIZES: &str = "state/build/sizes.json";

/// A unit's outputs that map tiles, the road index and rail stops read: its base pack, road values
/// and roads' English (a prune of a unit drops them; its job also writes analysis grids, which
/// none of those reads).
pub const UNIT_OUTPUTS: [&str; 4] = ["base/", "global/roads/", "global/roaden/", ROAD_IDS];

/// A unit's road ids (its job's), sorted, each once: what the road → units index reads of it.
pub const ROAD_IDS: &str = "global/roadids/";

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
/// The records' manifest under `root` (`state/build/manifest.json`), read only: the accepted
/// versions, for a reader that needs no `Out` (coord::hosts).
pub fn read_manifest(root: &store::nasfs::NasRoot) -> Result<BTreeMap<String, String>> {
    read_record(&root.join("state/build/manifest.json"))
}

pub fn read_record<T: Default + serde::de::DeserializeOwned>(p: &NasPath) -> Result<T> {
    match p.read_replaced() {
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
    pub fn open(root: &store::nasfs::NasRoot, scratch: &Path, step: &str) -> Result<Self> {
        let mut out = Self::open_here(root, scratch)?;
        out.for_step(step);
        Ok(out)
    }

    /// Not a step's job's: the agent merging hand-offs, a tool, a test of a step's parts. It may
    /// put and remove any name, so a step's job opens with `open`, never this.
    pub fn unbound(root: &store::nasfs::NasRoot, scratch: &Path) -> Result<Self> {
        Self::open_here(root, scratch)
    }

    /// The records at `root`, with this Mac's unmerged hand-offs its agent names
    /// (crate::agent::resume::UNMERGED_ENV).
    fn open_here(root: &store::nasfs::NasRoot, scratch: &Path) -> Result<Self> {
        let unmerged = std::env::var_os(crate::agent::resume::UNMERGED_ENV).map(PathBuf::from);
        Self::open_with(root, scratch, unmerged.as_deref())
    }

    /// The records at `root`, with this Mac's hand-offs for the job's step its lead hasn't merged
    /// (`unmerged`: a file of one hand-off, their changes laid in order, made by its agent with
    /// `state/pool/resume` on: crate::agent::resume) laid over the snapshot, as
    /// `Keys::load_with` lays the done records: a job resuming its own work (the OSM pass after
    /// sleep) finds its earlier stages' outputs (docs/pool.md §7.4). Its agent gives them only to a
    /// job that hands off (`SCENIC_HANDOFF`): one that saves the records has none. They're the snapshot's,
    /// not this job's changes: its saves hand off only its own.
    pub fn open_with(root: &store::nasfs::NasRoot, scratch: &Path, unmerged: Option<&Path>) -> Result<Self> {
        let _p = crate::timings::phase("records read", crate::timings::Class::NasRead);
        std::fs::create_dir_all(scratch)?;
        let manifest_path = root.join("state/build/manifest.json");
        let manifest: BTreeMap<String, String> = read_record(&manifest_path)?;
        let handoff = std::env::var_os("SCENIC_HANDOFF").map(PathBuf::from);
        let mut manifest = manifest;
        if let Some(p) = unmerged {
            let b = std::fs::read(p).with_context(|| format!("read this Mac's unmerged hand-offs, {}", p.display()))?;
            let h: crate::handoff::Handoff =
                serde_json::from_slice(&b).with_context(|| format!("parse {}", p.display()))?;
            lay_over(&mut manifest, &h);
        }
        // (A helper's job hands off only its own uploads.)
        let pending =
            if handoff.is_some() { BTreeMap::new() } else { read_record(&root.join("state/build/pending.json"))? };
        Ok(Out {
            root: root.clone(),
            manifest,
            changes: BTreeMap::new(),
            checked: Default::default(),
            pending,
            sizes: BTreeMap::new(),
            manifest_path,
            scratch: scratch.to_path_buf(),
            handoff,
            step: None,
            outside: Vec::new(),
            later: Later::default(),
            uploaded: BTreeMap::new(),
        })
    }

    /// Binds this to a job of `step` (scenic-build, as it starts a step): from now on it puts and
    /// removes only `step`'s outputs (crate::agent::steps::writes, the function the lead's merge
    /// checks every entry against, so the writer and the check can't disagree; docs/pool.md §7.3).
    /// A put outside them fails before anything is uploaded; a `set` or `remove` outside them fails
    /// the next save, and nothing of that save lands (what earlier saves saved, the targets done
    /// before it, stands). A step the steps table doesn't know (a tool run by
    /// hand) is held to nothing while it writes the manifest, and changes nothing when it hands off
    /// (the merge refuses such an entry).
    /// A job's that hands off to `handoff`, as `SCENIC_HANDOFF` makes it (tests: the environment is
    /// the whole process's).
    #[cfg(test)]
    pub fn handing_off(mut self, handoff: &Path) -> Self {
        self.handoff = Some(handoff.to_path_buf());
        self.pending = BTreeMap::new();
        self
    }

    fn for_step(&mut self, step: &str) {
        self.step = Some(step.to_string());
    }

    /// Whether this job may put (`removal`: remove) `logical`; why not.
    fn admits(&self, logical: &str, removal: bool) -> Result<()> {
        let Some(step) = &self.step else { return Ok(()) };
        match crate::agent::steps::writes(step, logical, removal) {
            Some(true) => Ok(()),
            Some(false) => bail!(
                "{} {logical}, outside {step}'s outputs (crate::agent::steps)",
                if removal { "removes" } else { "writes" }
            ),
            None if self.handoff.is_some() => bail!(
                "{} {logical}: {step} isn't a step the steps table knows, so its hand-off may change nothing",
                if removal { "removes" } else { "writes" }
            ),
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

    pub fn root(&self) -> &store::nasfs::NasRoot {
        &self.root
    }

    /// The NAS path of a content name. A path in an inputs folder (`inputs/`, `translations/`,
    /// `descriptions/`) is refused, naming the caller: a step reads an input's accepted version
    /// through `crate::inputs::open` alone (docs/inputs.md §7.1), so a reader missed fails its
    /// first run instead of bypassing the gate.
    #[track_caller]
    pub fn path(&self, content: &str) -> NasPath {
        if crate::inputs::in_inputs_folder(content) {
            panic!("{}: {content} is in an inputs folder; a step reads an input through crate::inputs::open alone (docs/inputs.md §7.1)", std::panic::Location::caller());
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
        self.settle_name(logical)?;
        let size = std::fs::metadata(local)?.len();
        let (h, sha) = hashes_of(local)?;
        let name = store::naming::content_name(logical, &h, ext);
        if self.uploaded.get(logical) == Some(&name) {
            // Uploaded under it by this very run already: nothing to look at again.
            std::fs::remove_file(local).ok();
            return Ok(self.landed(Landed { logical: logical.to_string(), name, sha: None, size, ms: 0 }));
        }
        if self.manifest.get(logical) == Some(&name) {
            // The records name this very file under it: one look (task #449: heritage-sites'
            // slices, hundreds of small files nearly all unchanged). There at its size: touched
            // (GC's age rule: a snapshot's name may have been dropped since) and taken. Else
            // `upload` as for any file: missing, put again; of another size, the put fails (a
            // damaged content-named file is removed by hand: store::naming); not touchable, taken
            // as an upload of its own, its check pending (nothing written).
            let dest = self.root.join(&name);
            let here = dest.metadata().is_ok_and(|m| m.len() == size)
                && dest.set_modified(std::time::SystemTime::now()).is_ok();
            if here {
                std::fs::remove_file(local).ok();
                return Ok(self.landed(Landed { logical: logical.to_string(), name, sha: None, size, ms: 0 }));
            }
        }
        let landed = upload_hashed(&self.root, logical, ext, local, (h, sha), size)?;
        self.uploaded.insert(landed.logical.clone(), landed.name.clone());
        if landed.sha.is_some() {
            crate::costin::wrote(&self.root.join(&landed.name), landed.size, landed.ms);
        }
        Ok(self.landed(landed))
    }

    /// The uploads put off up to the last of `logical` landed, when it's in flight: so a later
    /// put, set or removal of it isn't undone when that upload lands.
    fn settle_name(&mut self, logical: &str) -> Result<()> {
        if let Some(k) = self.later.sizes.iter().rposition(|(_, l)| l == logical) {
            self.settle(Mark(self.later.landed + k as u64 + 1))?;
        }
        Ok(())
    }

    /// Every upload put off that has landed, claimed into the target under way's measures
    /// (crate::costin's `wrote` and `creates`): a step whose target saves its own uploads.
    pub fn claim_into_costin(&mut self) {
        let (b, n) = self.claim_written(self.mark());
        crate::costin::add("wrote", b);
        crate::costin::add("creates", n);
    }

    /// The bytes copied (and files made) by the uploads put off before `upto` that have landed,
    /// claimed once: what the target that put them off adds to its cost line (`wrote`, `creates`).
    pub fn claim_written(&mut self, upto: Mark) -> (u64, u64) {
        let (mine, rest): (Vec<_>, Vec<_>) = self.later.unclaimed.iter().partition(|(seq, _)| *seq < upto.0);
        self.later.unclaimed = rest;
        (mine.iter().map(|(_, b)| b).sum(), mine.len() as u64)
    }

    /// An upload made (`upload`), taken into this run's records: named in the manifest, its check
    /// pending when it was copied, its bytes counted to the phase it's taken in (its target's cost
    /// line: `put_file` now, `claim_written` for one put off).
    fn landed(&mut self, l: Landed) -> String {
        let made = if l.sha.is_some() { l.size } else { 0 };
        crate::timings::count(made, 1);
        if let Some(sha) = l.sha {
            self.pending.insert(l.name.clone(), sha);
        }
        self.sizes.insert(l.name.clone(), l.size);
        self.manifest.insert(l.logical.clone(), l.name.clone());
        self.changes.insert(l.logical, Some(l.name.clone()));
        l.name
    }

    /// `put_file`, put off: the file is uploaded on a thread beside the job (in turn with the
    /// others put off) while the job goes on, and named in the manifest once it has landed
    /// (`settle`). Until then `get` doesn't name it. A save waits for every upload put off before
    /// it (`save`), or those up to a mark (`save_upto`), and fails if one did: a target is noted
    /// done only after the save that names its outputs, so never before they're on the NAS. Checked
    /// against the step's outputs now, as `put_file` is. A `put_file`, `set` or `remove` of a name
    /// whose upload is in flight waits for it to land first, so it never lands over newer state.
    /// Its bytes go to no cost line by themselves: the target claims them (`claim_written`).
    pub fn put_file_later(&mut self, logical: &str, ext: &str, local: &Path) -> Result<()> {
        if logical.contains('.') {
            bail!("logical names have no dots: {logical}");
        }
        self.admits(logical, false)?;
        let size = std::fs::metadata(local)?.len();
        if self.later.to.is_none() {
            let (to, rx) = std::sync::mpsc::channel::<Upload>();
            let (tx, back) = std::sync::mpsc::channel::<Result<Landed>>();
            let spawned = std::thread::Builder::new().name("uploads".into()).spawn(move || {
                for u in rx {
                    if tx.send(upload_retrying(&u.root, &u.logical, &u.ext, &u.local, UPLOAD_WAITS)).is_err() {
                        return;
                    }
                }
            });
            if spawned.is_err() {
                // (No threads: the WebAssembly builds. Uploaded now.)
                self.put_file(logical, ext, local)?;
                return Ok(());
            }
            self.later.to = Some(to);
            self.later.back = Some(std::sync::Mutex::new(back));
        }
        let u = Upload {
            root: self.root.clone(),
            logical: logical.to_string(),
            ext: ext.to_string(),
            local: local.to_path_buf(),
        };
        self.later
            .to
            .as_ref()
            .unwrap()
            .send(u)
            .map_err(|_| anyhow::anyhow!("{logical}: the uploads' thread is gone"))?;
        self.later.sent += 1;
        self.later.sizes.push_back((size, logical.to_string()));
        Ok(())
    }

    /// The uploads put off so far: `save_upto` this waits for these alone.
    pub fn mark(&self) -> Mark {
        Mark(self.later.sent)
    }

    /// Waits for the uploads put off up to `upto` to land, each within its deadline
    /// (`upload_deadline`), and takes them into the records. The first that failed, or didn't land
    /// in time, fails this (and the job: what it put off isn't named).
    fn settle(&mut self, upto: Mark) -> Result<()> {
        if let Some(f) = &self.later.failed {
            bail!("an upload put off failed earlier: {f}");
        }
        if self.later.landed >= upto.0 {
            return Ok(());
        }
        let _p = crate::timings::phase("uploads put off landed", crate::timings::Class::NasWrite);
        while self.later.landed < upto.0 {
            let size = self.later.sizes.front().map_or(0, |s| s.0);
            let deadline = upload_deadline(size);
            let got = self.later.back.as_ref().expect("an upload sent").lock().unwrap().recv_timeout(deadline);
            let l = match got {
                Ok(Ok(l)) => l,
                Ok(Err(e)) => {
                    let e = e.context("an upload put off failed");
                    self.later.failed = Some(format!("{e:#}"));
                    return Err(e);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    let e = anyhow::anyhow!(
                        "an upload put off ({size} bytes) didn't land in {} s (the NAS's share stuck?)",
                        deadline.as_secs()
                    );
                    self.later.failed = Some(format!("{e:#}"));
                    return Err(e);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    self.later.failed = Some("the uploads' thread is gone".into());
                    bail!("the uploads' thread is gone");
                }
            };
            self.later.sizes.pop_front();
            let seq = self.later.landed;
            self.later.landed += 1;
            if l.sha.is_some() {
                // (Its NAS write counted as it lands; its cost line's as its target claims it.)
                self.root.join(&l.name).count_wrote(l.size, l.ms);
                self.later.unclaimed.push((seq, l.size));
            }
            self.uploaded.insert(l.logical.clone(), l.name.clone());
            self.landed(l);
        }
        Ok(())
    }

    /// `put_file`, saying how far it is to `on` about once a second, as (bytes, total): it reads the
    /// file twice (its name's hash and its checksum; the copy) and the copy once more on the NAS, for
    /// a large file most of a step's time.
    pub fn put_file_with(
        &mut self,
        logical: &str,
        ext: &str,
        local: &Path,
        on: &(dyn Fn(u64, u64) + Sync),
    ) -> Result<String> {
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
        let t = std::time::Instant::now();
        let got = store::naming::write_atomic_hashed(&self.root, logical, ext, &local, &h, bytes.len() as u64);
        if got.is_ok() && existed {
            let touched = dest.set_modified(std::time::SystemTime::now()).is_ok();
            if !touched || !dest.exists() {
                existed = false;
                let again =
                    store::naming::write_atomic_hashed(&self.root, logical, ext, &local, &h, bytes.len() as u64);
                std::fs::remove_file(&local).ok();
                anyhow::ensure!(again? == name, "{logical}: wrote another name than {name}");
            }
        }
        std::fs::remove_file(&local).ok();
        anyhow::ensure!(got? == name, "{logical}: wrote another name than {name}");
        crate::timings::count(if existed { 0 } else { bytes.len() as u64 }, 1);
        if !existed {
            crate::costin::wrote(&dest, bytes.len() as u64, t.elapsed().as_millis() as u64);
        }
        self.sizes.insert(name.clone(), bytes.len() as u64);
        if !existed {
            self.pending.insert(name.clone(), format!("{:x}", sha2::Sha256::digest(bytes)));
        }
        Ok(name)
    }

    /// Names `content` (None: nothing) under `logical` in the manifest: a file uploaded already
    /// (`store_bytes`).
    pub fn set(&mut self, logical: &str, content: Option<String>) {
        // (One in flight landing fails: the next save fails with it.)
        let _ = self.settle_name(logical);
        self.uploaded.remove(logical);
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
        // (One in flight landing fails: the next save fails with it.)
        let _ = self.settle_name(logical);
        self.uploaded.remove(logical);
        self.note(logical, true);
        self.manifest.remove(logical);
        self.changes.insert(logical.to_string(), None);
    }

    /// Drops content name `c` from the unverified uploads (`pending`) at the next save, as a check
    /// would: a removed step's file GC drops (crate::agent::gc::drop_names), never verified now.
    pub fn unpend(&mut self, c: &str) {
        self.checked.insert(c.to_string());
    }

    /// Writes this run's changes into the manifest on disk (re-read under a lock, so another step's
    /// changes saved meanwhile are kept), and the unverified uploads likewise. A helper's job hands
    /// them off instead (crate::handoff): in the pool, every job's (`check_writer`).
    pub fn save(&mut self) -> Result<()> {
        self.save_upto(self.mark())
    }

    /// `save`, after the uploads put off up to `upto` have landed (`settle`): those after it, still
    /// on their way, are left for a later save (a target's, while the next one's are uploading).
    pub fn save_upto(&mut self, upto: Mark) -> Result<()> {
        self.settle(upto)?;
        let _p = crate::timings::phase("records saved", crate::timings::Class::NasWrite);
        // (Nothing saved of a job that named something outside its step's outputs.)
        if let Some(first) = self.outside.first() {
            bail!(
                "{first}{}: nothing saved",
                if self.outside.len() > 1 { format!(" (and {} more)", self.outside.len() - 1) } else { String::new() }
            );
        }
        if let Some(dir) = &self.handoff {
            if self.changes.is_empty() && self.pending.is_empty() && self.checked.is_empty() {
                return Ok(());
            }
            let h = crate::handoff::Handoff {
                changes: self.changes.clone(),
                pending: self.pending.clone(),
                checked: self.checked.iter().cloned().collect(),
                sizes: self.named_sizes(),
                done: None,
                raw: Vec::new(),
            };
            crate::handoff::write(dir, &h)?;
            self.sizes.retain(|c, _| !h.sizes.contains_key(c));
            self.changes.clear();
            self.pending.clear();
            self.checked.clear();
            return Ok(());
        }
        check_writer(&self.root)?;
        let lock = BuildLock::take(&self.root)?;
        self.save_held(&lock)
    }

    /// Takes in a hand-off's changes (the lead's agent merging them: crate::handoff::merge).
    pub fn absorb(&mut self, h: &crate::handoff::Handoff) {
        for (k, v) in &h.changes {
            match v {
                Some(n) => self.manifest.insert(k.clone(), n.clone()),
                None => self.manifest.remove(k),
            };
            self.changes.insert(k.clone(), v.clone());
        }
        self.pending.extend(h.pending.clone());
        self.sizes.extend(h.sizes.clone());
        self.checked.extend(h.checked.iter().cloned());
    }

    /// `save`, under this Mac's build lock, held. (A record its changes leave as it is isn't
    /// written again: a step's last save often has none, and each write is a rename over a file
    /// another Mac may have open.)
    pub fn save_held(&mut self, _lock: &BuildLock) -> Result<()> {
        let dir = self.manifest_path.parent().unwrap();
        dir.create_dir_all()?;
        let read: BTreeMap<String, String> = read_record(&self.manifest_path)?;
        let mut on_disk = read.clone();
        for (k, v) in &self.changes {
            match v {
                Some(n) => on_disk.insert(k.clone(), n.clone()),
                None => on_disk.remove(k),
            };
        }
        // (The sizes of the files it names, those of names no longer in it let go.)
        let sizes_path = self.root.join(SIZES);
        let sizes_read: BTreeMap<String, u64> = read_record(&sizes_path)?;
        let mut sizes = sizes_read.clone();
        sizes.extend(self.named_sizes());
        let named: std::collections::BTreeSet<&String> = on_disk.values().collect();
        sizes.retain(|c, _| named.contains(c));
        let pending_path = dir.join("pending.json");
        let pending_read: BTreeMap<String, String> = read_record(&pending_path)?;
        let mut pending = pending_read.clone();
        pending.extend(self.pending.clone());
        pending.retain(|k, _| !self.checked.contains(k));
        for (p, v, was) in [(&self.manifest_path, &on_disk, &read), (&pending_path, &pending, &pending_read)] {
            if v != was || !p.exists() {
                p.write_whole(serde_json::to_vec_pretty(v)?)?;
            }
        }
        if sizes != sizes_read {
            sizes_path.write_whole(serde_json::to_vec(&sizes)?)?;
        }
        let handed = self.named_sizes();
        self.sizes.retain(|c, _| !handed.contains_key(c));
        self.manifest = on_disk;
        self.pending = pending;
        self.changes.clear();
        self.checked.clear();
        Ok(())
    }

    /// The sizes of this run's files that its changes name (the others, `store_bytes`'s not named
    /// yet, wait for a later save).
    fn named_sizes(&self) -> BTreeMap<String, u64> {
        let named: std::collections::BTreeSet<&String> = self.changes.values().flatten().collect();
        self.sizes.iter().filter(|(c, _)| named.contains(c)).map(|(c, n)| (c.clone(), *n)).collect()
    }

    /// The sizes the records hold of the files the manifest names (`SIZES`), with this run's own:
    /// what a catalogue lists them as, never looking at the NAS for them. An error when the record
    /// can't be read now; a file it lacks (written before sizes were recorded) isn't in it.
    pub fn recorded_sizes(&self) -> Result<BTreeMap<String, u64>> {
        let mut s: BTreeMap<String, u64> = read_record(&self.root.join(SIZES))?;
        s.extend(self.sizes.iter().map(|(c, n)| (c.clone(), *n)));
        Ok(s)
    }
}

/// How long one hashing call (`verify`) may take: three minutes (the call's own authentication, and a
/// NAS busy with others' reads), and a second for each 16 MB it hashes (the NAS's disks and CPU
/// give far more; this is the rate it would have to fall below to be stopped). Its floor and
/// grace are short in a test.
pub fn verify_deadline(bytes: u64) -> std::time::Duration {
    std::time::Duration::from_secs(if cfg!(test) { 1 } else { 180 } + (bytes >> 24))
}

/// How long a call past its deadline has to end after SIGTERM before it's killed.
const VERIFY_GRACE: std::time::Duration =
    if cfg!(test) { std::time::Duration::from_millis(300) } else { std::time::Duration::from_secs(10) };

/// `store::spawn::output` held to `deadline`: past it the child (and the group it leads) gets
/// SIGTERM, and SIGKILL when it hasn't ended `grace` later; the error is `TimedOut`. Nothing else is
/// signalled: its group is its own.
fn output_within(
    cmd: &mut std::process::Command,
    deadline: std::time::Duration,
    grace: std::time::Duration,
) -> std::io::Result<std::process::Output> {
    use std::io::Read;
    use std::process::Stdio;
    crate::sys::own_group(cmd);
    let mut child = store::spawn::spawn(cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()))?;
    let pgid = child.id() as i32;
    let drain = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut b = Vec::new();
            if let Some(mut r) = r {
                r.read_to_end(&mut b).ok();
            }
            b
        })
    };
    let o = drain(child.stdout.take().map(|r| Box::new(r) as _));
    let e = drain(child.stderr.take().map(|r| Box::new(r) as _));
    let t = std::time::Instant::now();
    // (Signals only while the child is unreaped: its PID, the group's id, can't be reused till
    // try_wait reaps it, and the loop ends there. Each signal goes once.)
    let (mut stopped, mut killed) = (false, false);
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if !stopped && t.elapsed() > deadline {
            stopped = true;
            crate::sys::signal_group(pgid, crate::sys::Signal::Term);
        } else if stopped && !killed && t.elapsed() > deadline + grace {
            killed = true;
            crate::sys::signal_group(pgid, crate::sys::Signal::Kill);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let (stdout, stderr) = (o.join().unwrap_or_default(), e.join().unwrap_or_default());
    if stopped {
        return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "stopped past its deadline"));
    }
    Ok(std::process::Output { status, stdout, stderr })
}

/// Checks unverified uploads `uploads` (content name → SHA-256) on the NAS itself: SHA-256 computed
/// there over SSH (`ssh` runs a command on the NAS; `nas_root` is the project folder as the NAS sees
/// it). The names that match, and those that don't or aren't there; nothing is removed or written
/// (GC's job takes the matching ones out of the records' pending: crate::agent::gc::verify_pending).
pub fn verify(
    ssh: &[&str],
    nas_root: &str,
    uploads: &BTreeMap<String, String>,
    bytes: u64,
) -> Result<(Vec<String>, Vec<String>)> {
    if uploads.is_empty() {
        return Ok(Default::default());
    }
    // (A first call, bounded: it authenticates, which may wait on the SSH agent unbounded
    // (osmpass::SSH_START), and opens the shared connection the hashes' calls then take.)
    #[cfg(not(target_os = "wasi"))]
    crate::osmpass::ssh_opened(ssh)?;
    let mut cmd = store::spawn::command(ssh[0]);
    cmd.args(&ssh[1..]);
    let quoted: Vec<String> = uploads.keys().map(|n| format!("'{}'", n.replace('\'', "'\\''"))).collect();
    // (A file not there: sha256sum says so on stderr and goes on; it's among those that don't match.)
    cmd.arg(format!("cd '{nas_root}' && {{ sha256sum {} 2>/dev/null; true; }}", quoted.join(" ")));
    // (The call itself bounded too: its authentication, outside the probe's, may stall on the SSH
    // agent again, and a `sha256sum` on the NAS may hang; past `verify_deadline` it is stopped.)
    let deadline = verify_deadline(bytes);
    let out = output_within(&mut cmd, deadline, VERIFY_GRACE).map_err(|e| {
        if e.kind() == std::io::ErrorKind::TimedOut {
            anyhow::Error::new(e)
                .context(format!("ssh to the NAS hashing {bytes} bytes: over {} s", deadline.as_secs()))
        } else {
            anyhow::Error::new(e).context("ssh to the NAS")
        }
    })?;
    anyhow::ensure!(out.status.success(), "ssh to the NAS: {}", String::from_utf8_lossy(&out.stderr).trim());
    let text = String::from_utf8_lossy(&out.stdout);
    let got: BTreeMap<&str, &str> = text.lines().filter_map(|l| l.split_once("  ")).map(|(h, n)| (n, h)).collect();
    let (ok, bad): (Vec<_>, Vec<_>) = uploads.iter().partition(|(n, h)| got.get(n.as_str()) == Some(&h.as_str()));
    Ok((ok.into_iter().map(|(n, _)| n.clone()).collect(), bad.into_iter().map(|(n, _)| n.clone()).collect()))
}

/// An error once the pool has begun (its first term made): the records are its lead's to write,
/// through its merge of the journal alone, and every job hands its saves off (SCENIC_HANDOFF,
/// docs/pool.md §7.3), whatever Mac it runs on. A folder no term was made in yet (a test's, a
/// scratch build's) has its records written as today's files (term 1's first snapshot is made from
/// them).
/// Uploads local file `local` under `logical` to `root` (`Out::put_file`), the local file removed
/// after: its content name, and its SHA-256 when it was copied. An identical file already on the
/// NAS is reused (and touched), not copied again; one of the same name with another size is an
/// error (content-named files are never rewritten).
fn upload(root: &store::nasfs::NasRoot, logical: &str, ext: &str, local: &Path) -> Result<Landed> {
    // (Its name's hash and its checksum from one read of it: `write_atomic_hashed` checks the
    // read-back against the hash, as against its own.)
    let size = std::fs::metadata(local)?.len();
    upload_hashed(root, logical, ext, local, hashes_of(local)?, size)
}

/// `upload` of a file whose hashes (`hashes_of`) and size are known.
fn upload_hashed(
    root: &store::nasfs::NasRoot,
    logical: &str,
    ext: &str,
    local: &Path,
    (h, sha): (String, String),
    size: u64,
) -> Result<Landed> {
    let name = store::naming::content_name(logical, &h, ext);
    let dest = root.join(&name);
    let mut existed = dest.exists();
    let t = std::time::Instant::now();
    let got = store::naming::write_atomic_hashed(root, logical, ext, local, &h, size)?;
    anyhow::ensure!(got == name, "{logical}: wrote {got}, expected {name}");
    if existed {
        // In use again: a fresh time keeps GC's age rule from taking it before a catalogue does
        // (GC looks at a file's time again right before it removes it). One that can't be
        // touched, or is gone once touched (GC took it between), is uploaded again.
        let touched = dest.set_modified(std::time::SystemTime::now()).is_ok();
        if !touched || !dest.exists() {
            existed = false;
            let again = store::naming::write_atomic_hashed(root, logical, ext, local, &h, size)?;
            anyhow::ensure!(again == name, "{logical}: wrote {again}, expected {name}");
        }
    }
    std::fs::remove_file(local).ok();
    let ms = t.elapsed().as_millis() as u64;
    Ok(Landed { logical: logical.to_string(), name, sha: (!existed).then_some(sha), size, ms })
}

/// `upload`, tried again after each of `waits` while it fails transiently
/// (store::retry::transient_or_missing: an SMB moment of ENOENT, EIO, a reset connection), each retry
/// logged; its last error returned.
fn upload_retrying(
    root: &store::nasfs::NasRoot,
    logical: &str,
    ext: &str,
    local: &Path,
    waits: &[std::time::Duration],
) -> Result<Landed> {
    let mut tries = 0;
    loop {
        match upload(root, logical, ext, local) {
            Ok(l) => return Ok(l),
            Err(e)
                if tries < waits.len()
                    && local.exists()
                    && e.chain().any(|c| {
                        c.downcast_ref::<std::io::Error>().is_some_and(store::retry::transient_or_missing)
                    }) =>
            {
                eprintln!("{logical}: upload: {e:#}; again in {:?}", waits[tries]);
                std::thread::sleep(waits[tries]);
                tries += 1;
            }
            Err(e) => return Err(e.context(format!("upload {logical}"))),
        }
    }
}

pub fn check_writer(root: &store::nasfs::NasRoot) -> Result<()> {
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
    fn file(root: &store::nasfs::NasRoot) -> PathBuf {
        // By the root's real path: `x`, `x/` and a link to it are one build.
        let raw = root.unchecked_path();
        let real = raw.canonicalize().unwrap_or(raw);
        let key = store::naming::hash16(real.to_string_lossy().as_bytes());
        std::env::temp_dir().join(format!("scenic-build-{key}.lock"))
    }

    /// Waits until the lock is ours.
    pub fn take(root: &store::nasfs::NasRoot) -> Result<BuildLock> {
        let l = crate::sys::PathLock::take(&Self::file(root), true).context("lock the build manifest")?;
        Ok(BuildLock(l.context("lock the build manifest: not taken")?))
    }

    /// The lock when it's free now; None when another holds it (a paused job may, for hours).
    pub fn try_take(root: &store::nasfs::NasRoot) -> Result<Option<BuildLock>> {
        Ok(crate::sys::PathLock::take(&Self::file(root), false).context("lock the build manifest")?.map(BuildLock))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Task #400: an upload put off lands as `put_file`'s would (the same name, size and pending
    /// check), named only once it has: a save waits for those before it (or up to a mark), and one
    /// that failed fails the save, unnamed, its local file kept.
    #[test]
    fn uploads_put_off_are_named_once_landed_as_put_files_are() {
        let d = tempfile::tempdir().unwrap();
        let file = |name: &str, bytes: &[u8]| {
            let p = d.path().join("made").join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, bytes).unwrap();
            p
        };
        let records = |root: &store::nasfs::NasRoot| {
            let m: BTreeMap<String, String> = read_record(&root.join("state/build/manifest.json")).unwrap();
            let p: BTreeMap<String, String> = read_record(&root.join("state/build/pending.json")).unwrap();
            let s: BTreeMap<String, u64> = read_record(&root.join(SIZES)).unwrap();
            (m, p, s)
        };
        // Now and put off: the same records.
        let (now, later) = (d.path().join("now"), d.path().join("later"));
        let mut a = Out::unbound(&store::nasfs::NasRoot::local_for_test(&now), &d.path().join("s1")).unwrap();
        a.put_file("base/6-1-1", "base", &file("a1", b"one")).unwrap();
        a.put_file("base/6-1-2", "base", &file("a2", b"two two")).unwrap();
        a.save().unwrap();
        let mut b = Out::unbound(&store::nasfs::NasRoot::local_for_test(&later), &d.path().join("s2")).unwrap();
        let first = file("b1", b"one");
        b.put_file_later("base/6-1-1", "base", &first).unwrap();
        let m1 = b.mark();
        b.put_file_later("base/6-1-2", "base", &file("b2", b"two two")).unwrap();
        assert_eq!(b.get("base/6-1-1"), None, "not named before it has landed");
        b.save_upto(m1).unwrap();
        assert!(
            records(&store::nasfs::NasRoot::local_for_test(&later)).0.contains_key("base/6-1-1"),
            "up to the mark: saved"
        );
        assert!(!first.exists(), "its local file removed once uploaded");
        b.save().unwrap();
        assert_eq!(
            records(&store::nasfs::NasRoot::local_for_test(&later)),
            records(&store::nasfs::NasRoot::local_for_test(&now))
        );
        assert_eq!(b.manifest, a.manifest);
        // Reused (already there): named, nothing pending for it.
        b.put_file_later("base/6-1-3", "base", &file("b3", b"one")).unwrap();
        b.save().unwrap();
        assert_eq!(b.get("base/6-1-3").map(|c| c.split('.').nth(1)), b.get("base/6-1-1").map(|c| c.split('.').nth(1)));
        // One that fails (where its folder would go, a file): the save fails, nothing named.
        std::fs::write(later.join("hidata"), b"in the way").unwrap();
        let kept = file("b4", b"four");
        b.put_file_later("hidata/6-1-4", "sect", &kept).unwrap();
        let e = b.save().unwrap_err();
        assert!(format!("{e:#}").contains("an upload put off failed"), "{e:#}");
        assert!(
            b.get("hidata/6-1-4").is_none()
                && !records(&store::nasfs::NasRoot::local_for_test(&later)).0.contains_key("hidata/6-1-4")
        );
        assert!(kept.exists(), "its local file kept");
        // Failed once, every later save fails, even with nothing new put off.
        assert!(b.save().is_err());
        // A name in flight put, set or removed: its upload lands first, never over the newer state.
        let mut c =
            Out::unbound(&store::nasfs::NasRoot::local_for_test(&d.path().join("c")), &d.path().join("s4")).unwrap();
        c.put_file_later("base/6-2-1", "base", &file("c1", b"old")).unwrap();
        c.remove("base/6-2-1");
        c.put_file_later("base/6-2-2", "base", &file("c2", b"old two")).unwrap();
        let newer = c.put_file("base/6-2-2", "base", &file("c3", b"newer")).unwrap();
        c.save().unwrap();
        assert_eq!(c.get("base/6-2-1"), None);
        assert_eq!(c.get("base/6-2-2"), Some(newer.as_str()));
        // Each upload's bytes claimed once, by the uploads before a mark.
        let m = c.mark();
        c.put_file_later("base/6-2-3", "base", &file("c4", b"four")).unwrap();
        c.save().unwrap();
        assert_eq!(c.claim_written(m), (10, 2), "the two put off before it (3 + 7 bytes)");
        assert_eq!(c.claim_written(c.mark()), (4, 1));
        assert_eq!(c.claim_written(c.mark()), (0, 0));
        // Outside the step's outputs: refused at once, as `put_file` is.
        let mut job = Out::open(&store::nasfs::NasRoot::local_for_test(&later), &d.path().join("s3"), "pack").unwrap();
        assert!(job.put_file_later("base/6-1-5", "base", &file("b5", b"five")).is_err());
        // Each a minute, and a second each 2 MB.
        assert_eq!(upload_deadline(0).as_secs(), 60);
        assert_eq!(upload_deadline(1 << 30).as_secs(), 60 + 512);
    }

    /// Task #449: a file the records already name under its logical name is looked at once: there
    /// at its size, touched and taken; missing or of another size, put back; one this run landed,
    /// not looked at again. Other bytes, or another name, are uploaded as before.
    #[test]
    fn a_file_the_records_name_is_checked_once_and_put_back_if_missing() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        let l = "work/heritage/d/pos/6-1-1";
        let mut out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("scratch")).unwrap();
        let a = out.put_bytes(l, "json", b"[[1,2]]").unwrap();
        out.save().unwrap();
        let open = || Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("scratch")).unwrap();
        // There: touched.
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86400);
        std::fs::File::options().write(true).open(root.join(&a)).unwrap().set_modified(old).unwrap();
        let mut out = open();
        assert_eq!(out.put_bytes(l, "json", b"[[1,2]]").unwrap(), a);
        let t = std::fs::metadata(root.join(&a)).unwrap().modified().unwrap();
        assert!(t.elapsed().unwrap() < std::time::Duration::from_secs(600), "touched");
        // Gone from the NAS (a GC after an old snapshot): put back.
        std::fs::remove_file(root.join(&a)).unwrap();
        let mut out = open();
        assert_eq!(out.put_bytes(l, "json", b"[[1,2]]").unwrap(), a);
        assert_eq!(std::fs::read(root.join(&a)).unwrap(), b"[[1,2]]");
        // Landed by this run (before its save): not looked at again (its copy gone meanwhile stays gone).
        std::fs::remove_file(root.join(&a)).unwrap();
        assert_eq!(out.put_bytes(l, "json", b"[[1,2]]").unwrap(), a);
        assert!(!root.join(&a).exists());
        // Other bytes, or another name: uploaded.
        let b = out.put_bytes(l, "json", b"[[3,4]]").unwrap();
        assert!(root.join(&b).exists());
        let c = out.put_bytes("work/heritage/d/pos/6-1-2", "json", b"[[1,2]]").unwrap();
        assert!(root.join(&c).exists());
    }

    /// Task #449: a file the records name, of another size on the NAS (cut short), is not taken: the
    /// put fails, as any upload over a damaged content-named file does (removed by hand).
    #[test]
    fn a_file_the_records_name_of_another_size_is_not_taken() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        let l = "work/heritage/d/pos/6-1-1";
        let mut out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("scratch")).unwrap();
        let a = out.put_bytes(l, "json", b"[[1,2],[3,4]]").unwrap();
        out.save().unwrap();
        std::fs::write(root.join(&a), b"[[1,2]").unwrap();
        let mut out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("scratch")).unwrap();
        assert!(out.put_bytes(l, "json", b"[[1,2],[3,4]]").is_err());
        assert_eq!(std::fs::read(root.join(&a)).unwrap(), b"[[1,2]", "left for the owner");
    }

    /// This Mac's unmerged hand-offs laid over the snapshot (docs/pool.md §7.4): their names read
    /// as the job's earlier attempts left them, none of them this job's own changes; none given,
    /// the snapshot as it is.
    /// What a job writes is recorded with its size: handed off with the changes naming it, or saved
    /// beside the manifest (`SIZES`), where the catalogue reads it (`recorded_sizes`); a name the
    /// manifest no longer has is let go.
    #[test]
    fn a_files_size_goes_with_the_change_naming_it() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        std::fs::create_dir_all(&root).unwrap();
        let mut out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("scratch")).unwrap();
        let a = out.put_bytes("base/6-1-1", "base", b"twelve bytes").unwrap();
        let unnamed = out.store_bytes("inputs-copy/x", "json", b"{}").unwrap();
        out.save().unwrap();
        let saved: BTreeMap<String, u64> =
            read_record(&store::nasfs::NasPath::local_for_test(&root.join(SIZES))).unwrap();
        assert_eq!(saved, [(a.clone(), 12)].into());
        assert_eq!(out.recorded_sizes().unwrap().get(&unnamed), Some(&2), "this run's own, named or not");
        out.remove("base/6-1-1");
        out.save().unwrap();
        assert!(read_record::<BTreeMap<String, u64>>(&store::nasfs::NasPath::local_for_test(&root.join(SIZES)))
            .unwrap()
            .is_empty());
        // A helper's job hands them off.
        let hd = d.path().join("handoff");
        let mut out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("scratch"))
            .unwrap()
            .handing_off(&hd);
        let b = out.put_bytes("base/6-1-2", "base", b"four").unwrap();
        out.save().unwrap();
        let h = crate::handoff::written_in(&hd).unwrap().unwrap();
        assert_eq!(h[0].sizes, [(b, 4)].into());
    }

    #[test]
    fn a_jobs_unmerged_hand_offs_are_laid_over_the_snapshot() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        std::fs::write(root.join("state/build/manifest.json"), r#"{"sources/osm/2026-09-01/pass": "sources/osm/2026-09-01/pass.1111111111111111.json", "sources/osm/2026-10-01/sets/old": "x"}"#).unwrap();
        let h = crate::handoff::Handoff {
            changes: [
                (
                    "sources/osm/2026-10-01/filtered".to_string(),
                    Some("sources/osm/2026-10-01/filtered.2222222222222222.osm.pbf".to_string()),
                ),
                ("sources/osm/2026-10-01/sets/old".to_string(), None),
            ]
            .into(),
            ..Default::default()
        };
        let f = d.path().join("unmerged.json");
        std::fs::write(&f, serde_json::to_vec(&h).unwrap()).unwrap();
        let mut out =
            Out::open_with(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("scratch"), Some(&f)).unwrap();
        out.for_step("osm-pass");
        assert_eq!(
            out.get("sources/osm/2026-10-01/filtered"),
            Some("sources/osm/2026-10-01/filtered.2222222222222222.osm.pbf")
        );
        assert!(
            out.get("sources/osm/2026-10-01/sets/old").is_none() && out.get("sources/osm/2026-09-01/pass").is_some()
        );
        assert!(out.changes.is_empty(), "the snapshot's, not this job's");
        let plain =
            Out::open_with(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("scratch"), None).unwrap();
        assert!(
            plain.get("sources/osm/2026-10-01/filtered").is_none()
                && plain.get("sources/osm/2026-10-01/sets/old").is_some()
        );
        // (A file that can't be read fails the job rather than open the snapshot without them.)
        assert!(Out::open_with(
            &store::nasfs::NasRoot::local_for_test(&root),
            &d.path().join("scratch"),
            Some(&d.path().join("missing.json"))
        )
        .is_err());
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
        let mut out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("scratch")).unwrap();
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
        // (A name the records dropped, put again: one they still name isn't looked at, task #449.)
        out.remove("work/x/6-1-1");
        assert_eq!(out.put_file("work/x/6-1-1", "bin", &local("b")).unwrap(), name);
        let t = std::fs::metadata(&dest).unwrap().modified().unwrap();
        assert!(t.elapsed().unwrap() < std::time::Duration::from_secs(600), "touched");
        assert!(out.pending.is_empty(), "reused, not uploaded");
        // Can't be touched (read-only here): taken as an upload of its own, its check pending.
        let mut perm = std::fs::metadata(&dest).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(&dest, perm).unwrap();
        out.remove("work/x/6-1-1");
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
        let mut out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("scratch")).unwrap();
        let name = out.store_bytes("sources/inputs/_gate-test/a", "jsonl", b"the same lines").unwrap();
        out.pending.clear();
        let dest = root.join(&name);
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86400);
        std::fs::File::options().write(true).open(&dest).unwrap().set_modified(old).unwrap();
        assert_eq!(out.store_bytes("sources/inputs/_gate-test/a", "jsonl", b"the same lines").unwrap(), name);
        assert!(
            std::fs::metadata(&dest).unwrap().modified().unwrap().elapsed().unwrap()
                < std::time::Duration::from_secs(600),
            "touched"
        );
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
        let held = BuildLock::take(&store::nasfs::NasRoot::local_for_test(d.path())).unwrap();
        let slashed = PathBuf::from(format!("{}/", d.path().display()));
        assert!(BuildLock::try_take(&store::nasfs::NasRoot::local_for_test(&slashed)).unwrap().is_none());
        drop(held);
        // (Free at once, however loaded the Mac: its file went with it, so a copy of its descriptor
        // a process another test is starting holds between its fork and its exec holds nothing:
        // crate::sys::PathLock.)
        assert!(
            BuildLock::try_take(&store::nasfs::NasRoot::local_for_test(&slashed)).unwrap().is_some(),
            "the lock wasn't free after it was dropped"
        );
    }

    #[test]
    fn records_unreadable_are_errors_not_empty() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("manifest.json");
        // None yet: empty.
        assert!(read_record::<BTreeMap<String, String>>(&store::nasfs::NasPath::local_for_test(&p))
            .unwrap()
            .is_empty());
        // Unreadable (a folder in its place, as an I/O error), or not JSON: errors.
        std::fs::create_dir(&p).unwrap();
        assert!(read_record::<BTreeMap<String, String>>(&store::nasfs::NasPath::local_for_test(&p)).is_err());
        std::fs::remove_dir(&p).unwrap();
        std::fs::write(&p, b"{not json").unwrap();
        assert!(read_record::<BTreeMap<String, String>>(&store::nasfs::NasPath::local_for_test(&p)).is_err());
        // A save with the manifest unreadable fails, and writes nothing over it.
        let root = d.path().join("root");
        let mut out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("s")).unwrap();
        out.changes.insert("a".into(), Some("a.1111111111111111.x".into()));
        std::fs::create_dir_all(root.join("state/build/manifest.json")).unwrap();
        assert!(out.save().is_err());
        assert!(root.join("state/build/manifest.json").is_dir());
        // And opening with it unreadable fails too.
        assert!(Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("s")).is_err());
    }

    #[test]
    fn a_save_with_nothing_new_writes_nothing_and_still_reads_what_others_saved() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("root");
        let mut out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("s")).unwrap();
        out.changes.insert("a".into(), Some("a.1111111111111111.x".into()));
        out.save().unwrap();
        let m = root.join("state/build/manifest.json");
        let written = std::fs::metadata(&m).unwrap().modified().unwrap();
        // (sleep ok: the manifest's times apart)
        std::thread::sleep(std::time::Duration::from_millis(20));
        // Nothing new: the records stay as they were (no rename over a file another Mac may have
        // open), and what another step saved meanwhile is read.
        let mut other = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("s2")).unwrap();
        other.changes.insert("b".into(), Some("b.2222222222222222.x".into()));
        other.save().unwrap();
        let theirs = std::fs::metadata(&m).unwrap().modified().unwrap();
        assert!(theirs > written);
        out.save().unwrap();
        assert_eq!(std::fs::metadata(&m).unwrap().modified().unwrap(), theirs);
        assert_eq!(out.get("b"), Some("b.2222222222222222.x"));
        // No temporary file left beside them.
        assert!(std::fs::read_dir(root.join("state/build"))
            .unwrap()
            .all(|e| !store::whole::is_tmp(&e.unwrap().path())));
    }

    #[test]
    fn once_the_pool_has_begun_no_step_saves_the_records() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("root");
        let mut out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("s")).unwrap();
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        // No term made: saved, as today's files.
        out.changes.insert("a".into(), Some("a.1111111111111111.x".into()));
        out.save().unwrap();
        assert!(root.join("state/build/manifest.json").exists());
        // The pool begun (a term made): no one's step writes them (a job hands off).
        std::fs::create_dir_all(root.join(crate::pool::term::DIR)).unwrap();
        out.changes.insert("b".into(), Some("b.2222222222222222.x".into()));
        assert!(format!("{:#}", out.save().unwrap_err()).contains("the pool leads this build"));
        assert!(check_writer(&store::nasfs::NasRoot::local_for_test(&root)).is_err());
    }

    /// A job puts and removes only its step's outputs (crate::agent::steps::writes, the function
    /// the lead's merge checks by): a put outside fails before it uploads, a set or removal outside
    /// fails the save before anything is saved, and a step the table doesn't know hands off nothing.
    #[test]
    fn a_job_writes_only_its_steps_outputs() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("root");
        let local = d.path().join("f.bin");
        let mut out = Out::open(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("s"), "pack").unwrap();
        // Its own: a map tile's data and road pack, a rail pack removed.
        std::fs::write(&local, b"tile").unwrap();
        out.put_file("hidata/6-1-2", "hidata", &local).unwrap();
        out.put_bytes("layers/roads/hi/6-1-2", "pack", b"roads").unwrap();
        out.remove("layers/rails/hi/6-1-2");
        out.save().unwrap();
        let saved: BTreeMap<String, String> =
            read_record(&store::nasfs::NasPath::local_for_test(&root.join("state/build/manifest.json"))).unwrap();
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
        assert!(
            e.contains("layers/terrain/hi/6-1-3") && e.contains("and 1 more") && e.contains("nothing saved"),
            "{e}"
        );
        assert_eq!(
            read_record::<BTreeMap<String, String>>(&store::nasfs::NasPath::local_for_test(
                &root.join("state/build/manifest.json")
            ))
            .unwrap(),
            saved
        );
        assert!(out.save().is_err(), "it stays failed");
        // A step the table doesn't know: held to nothing while it writes the manifest (a tool run
        // by hand) and to no change at all when it hands off (the merge refuses such an entry).
        let mut tool =
            Out::open(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("s2"), "patch-ferries").unwrap();
        tool.put_bytes("sources/osm/2026-09-28/pieces/6-1-2", "osm", b"p").unwrap();
        tool.handoff = Some(d.path().join("handoff"));
        assert!(format!("{:#}", tool.put_bytes("sources/osm/2026-09-28/pieces/6-1-3", "osm", b"q").unwrap_err())
            .contains("isn't a step the steps table knows"));
        // Not bound to a step (the agent merging hand-offs): anything.
        let mut merge = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("s3")).unwrap();
        merge.set("layers/terrain/hi/6-1-4", Some("layers/terrain/hi/6-1-4.2222222222222222.pack".into()));
        merge.save().unwrap();
    }
}
