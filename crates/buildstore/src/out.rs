//! Where build steps put their outputs: the NAS project folder (docs/storage.md), as immutable
//! content-named files, with a build manifest (logical name → file) the catalogue is made from.
//!
//! A file is first written locally (this Mac's SSD), hashed, then copied to `<name>.tmp` on
//! the NAS, read back past the client's cache and checked against its hash, and renamed into place
//! (`store::naming::write_atomic`). A file already there is reused as it is (one the manifest names
//! under the same logical name with one look: `put_file`) and named in the hand-off's `reused`, which
//! GC's queue and the merge go by (docs/storage.md, GC). An upload, copied or reused, stays pending
//! until `verify` checks it on the NAS itself (there at its size, by one call over SSH a batch: a
//! duty of the `gc` job).
//!
//! Several steps may run at once on the lead (the agent's job and a manual one): each records
//! its own changes and merges them into the manifest on disk under this Mac's lock (`BuildLock`)
//! when it saves. A helper's job on the other Mac (docs/building.md §2, The pool; `SCENIC_HANDOFF`)
//! saves its changes as hand-offs instead (crate::handoff), which the lead's agent merges: the
//! lead alone writes the manifest.
//!
//! The records an `Out` starts from are read here (crate::records, the pool's snapshot or today's
//! files); a step's write set is the agent's (pipeline::agent::steps::writes), handed to `open` as
//! a function (task #916 step 6a, docs/crates.md §6; pipeline::out::open passes it).

use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use store::nasfs::NasPath;

/// Whether step `step` may put (`removal`: remove) `logical`; None for a step the steps table
/// doesn't know (pipeline::agent::steps::writes, the function the lead's merge checks every entry
/// against, so the writer and the check can't disagree; docs/pool.md §7.3).
pub type Writes = fn(step: &str, logical: &str, removal: bool) -> Option<bool>;

/// Whether this process is a crate's unit tests (`go_testing`, from their start-up constructor;
/// buildstore's own from the start): an `Out`'s retries wait 1 ms (`upload_waits`), a call on
/// the NAS may be given a test's short deadline (`short_nas_calls`), and a disk's free space is the test's own
/// (`room::disk_free`). Never a feature's: an example, a binary or another
/// crate's test built with the dev-dependencies is no test of it (pipeline's tests/test_mode.rs).
pub fn testing() -> bool {
    TESTING.load(std::sync::atomic::Ordering::Relaxed)
}

/// Makes this process a unit tests' (`testing`): called only by a crate's `#[cfg(test)]` start-up
/// constructor (each crate's lib.rs `TEST_START`: pipeline's and its families').
pub fn go_testing() {
    TESTING.store(true, std::sync::atomic::Ordering::Relaxed);
}

static TESTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(cfg!(test));

/// A job's environment: the file of its member's unacknowledged hand-offs for its step
/// (`unmerged`), which `Out` lays over the snapshot (agentcore::agent::resume).
pub const UNMERGED_ENV: &str = "SCENIC_UNMERGED";

/// A job's environment: the records' brief its agent had from the lead as it started the job (a
/// local file, the lead's answer to `GET /pool/brief`, or its own coordinator's: docs/coordination.md
/// step 8d), which a job that hands off (`SCENIC_HANDOFF`) opens its manifest from in place of the
/// NAS's snapshot: never older than the records its lease was granted from. Not set (a lead on an
/// older app, or a job outside the pool): the NAS's, as before.
pub const BRIEF_ENV: &str = "SCENIC_BRIEF";

/// The outputs of a run.
pub struct Out {
    root: store::nasfs::NasRoot,
    /// Logical name → content name of every file this build wrote or reused.
    pub manifest: BTreeMap<String, String>,
    /// This run's changes to the manifest (None: removed), merged into the file when saving.
    changes: BTreeMap<String, Option<String>>,
    /// Uploads this run checked (no longer pending), likewise.
    checked: std::collections::BTreeSet<String>,
    /// Uploads not yet verified on the NAS (crate::pending).
    pending: crate::pending::Pending,
    /// Content name → size (bytes) of the files this run wrote or reused, handed on with the
    /// manifest changes that name them (`SIZES`, the catalogues' sizes without a look at the NAS).
    sizes: BTreeMap<String, u64>,
    manifest_path: NasPath,
    /// Local scratch space for files before upload.
    pub scratch: PathBuf,
    /// A helper's job: where its saves go as hand-offs ($SCENIC_HANDOFF), never the manifest.
    handoff: Option<PathBuf>,
    /// The step whose job this is, and its write set (`open`): it puts and removes only the names
    /// its outputs function gives, and a name outside them fails the job. None: no step (the agent
    /// merging hand-offs, a tool).
    step: Option<(String, Writes)>,
    /// Names set or removed outside the step's outputs: the save fails, naming the first.
    outside: Vec<String>,
    /// Uploads put off (`put_file_later`), made in turn on a thread beside the job.
    later: Later,
    /// Logical name → content name of each file this run uploaded (or reused there by `upload`)
    /// and landed, until set or removed: one put again unchanged isn't looked at on the NAS again
    /// (`put_file`).
    uploaded: BTreeMap<String, String>,
    /// The content names this run found on the NAS and took as they were, not yet handed off
    /// (crate::handoff::Handoff::reused: what GC's queue holds and the merge checks, as nothing
    /// touches a reused file).
    reused: std::collections::BTreeSet<String>,
}

/// The uploads put off (`Out::put_file_later`): sent to a thread of their own that makes them in
/// turn while the job goes on, their results back in the same order. Each is named in the manifest
/// only once it has landed (`Out::settle`), so a save never names a file that isn't on the NAS.
struct Later {
    to: Option<std::sync::mpsc::Sender<Upload>>,
    /// (In a mutex: an `Out` is shared by reference between threads.)
    back: Option<std::sync::Mutex<std::sync::mpsc::Receiver<Result<Landed>>>>,
    /// The bytes their thread has moved (store::naming's `count_here_into`): `settle` tells a
    /// moving upload from a stuck one by it, never blocking on the NAS itself.
    moved: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// The clock `settle` waits on (a test's virtual one).
    clock: std::sync::Arc<dyn store::clock::Clock>,
    /// The uploads sent and those landed (taken back): `Mark`s count them.
    sent: u64,
    landed: u64,
    /// Each sent upload's size and logical name not yet landed, in order (said when it stalls; a
    /// name in flight, which `put_file`, `set` and `remove` settle first).
    sizes: std::collections::VecDeque<(u64, String)>,
    /// The bytes copied by uploads landed and not yet claimed (`claim_written`), by their order
    /// sent: the target that put them off claims them for its cost line.
    unclaimed: Vec<(u64, u64)>,
    /// The first upload that failed or didn't land: every later save fails with it.
    failed: Option<String>,
}

impl Default for Later {
    fn default() -> Self {
        Later {
            to: None,
            back: None,
            moved: Default::default(),
            clock: store::clock::real(),
            sent: 0,
            landed: 0,
            sizes: Default::default(),
            unclaimed: Vec::new(),
            failed: None,
        }
    }
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
    /// Its check pending: copied, or found there already and taken as it was (a reuse is checked
    /// as an upload is: nothing touches it, docs/storage.md, GC). False: this run's own upload
    /// again, pending already.
    pending: bool,
    /// Whether it was copied (else found there already).
    copied: bool,
    size: u64,
    /// How long its copy took (ms; 0: none made).
    ms: u64,
}

/// The uploads put off up to a moment (`Out::mark`): `Out::save_upto` waits for those alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark(u64);

/// How long the uploads put off may go with no byte moved (hashed here, written to the share or
/// read back from it: store::naming's `count_moved`, counted by their thread) before the job
/// fails, the share taken as stuck: its thread is left to it, and the job's retry takes over. Not
/// a limit on an upload's whole time: a busy share with several Macs writing is slow but moves, and
/// lands. Two minutes: a moving upload's longest silence is one chunk written past the client's
/// cache (4 MB: a share at 50 KB/s still moves one inside it) or its retries' waits
/// (`upload_waits`, held under it below).
const UPLOAD_STALL: std::time::Duration = std::time::Duration::from_secs(120);

const _: () = {
    let (mut i, mut s) = (0, 0);
    let waits = upload_waits(false);
    while i < waits.len() {
        s += waits[i].as_millis();
        i += 1;
    }
    assert!(s < UPLOAD_STALL.as_millis(), "an upload's retry waits alone would read as a stall");
};

/// How often `settle` looks at the uploads while it waits for them (what it may land late by).
const UPLOAD_LOOK: std::time::Duration = std::time::Duration::from_millis(100);

/// The waits before a put-off upload that failed transiently is tried again (an SMB moment of
/// ENOENT, EIO or a reset connection): its file is content-named, so a retry lands the same file.
/// A millisecond in a test (`testing`).
const fn upload_waits(testing: bool) -> &'static [std::time::Duration] {
    const TEST: &[std::time::Duration] = &[std::time::Duration::from_millis(1)];
    const REAL: &[std::time::Duration] =
        &[std::time::Duration::from_secs(2), std::time::Duration::from_secs(5), std::time::Duration::from_secs(15)];
    if testing {
        TEST
    } else {
        REAL
    }
}

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
pub fn read_record<T: Default + serde::de::DeserializeOwned>(p: &NasPath) -> Result<T> {
    match p.read_replaced() {
        Ok(b) => serde_json::from_slice(&b).with_context(|| format!("parse {}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e).with_context(|| format!("read {}", p.display())),
    }
}

/// The most memory an upload holds as it goes (bytes; a step's memory bound counts one put off in
/// flight beside its next target's work): one buffer of store::naming's at a time (`CHUNK`): its
/// hash's read (`hash16_file`), its copy's and its read-back's, each freed before the next begins.
pub const UPLOAD_HELD: u64 = store::naming::CHUNK as u64;

/// The records as a resumed run's identity reads them (jobrt::resume_log::names).
impl jobrt::resume_log::Manifest for Out {
    fn get(&self, logical: &str) -> Option<&str> {
        Out::get(self, logical)
    }
}

impl Out {
    /// A job of `step`'s: `root`, the NAS project folder (or a local folder standing in for it);
    /// `scratch`, local space. It puts and removes only `step`'s outputs as `writes` gives them
    /// (`for_step`; pipeline::out::open passes the steps table's).
    pub fn open(root: &store::nasfs::NasRoot, scratch: &Path, step: &str, writes: Writes) -> Result<Self> {
        let mut out = Self::open_here(root, scratch)?;
        out.for_step(step, writes);
        Ok(out)
    }

    /// Not a step's job's: the agent merging hand-offs, a tool, a test of a step's parts. It may
    /// put and remove any name, so a step's job opens with `open`, never this.
    pub fn unbound(root: &store::nasfs::NasRoot, scratch: &Path) -> Result<Self> {
        Self::open_here(root, scratch)
    }

    /// The records at `root`, with this Mac's unmerged hand-offs its agent names (`UNMERGED_ENV`).
    fn open_here(root: &store::nasfs::NasRoot, scratch: &Path) -> Result<Self> {
        let unmerged = std::env::var_os(UNMERGED_ENV).map(PathBuf::from);
        let handoff = std::env::var_os("SCENIC_HANDOFF").map(PathBuf::from);
        let brief = std::env::var_os(BRIEF_ENV).map(PathBuf::from);
        Self::open_from(root, scratch, unmerged.as_deref(), handoff, brief.as_deref())
    }

    /// The records at `root`, with this Mac's hand-offs for the job's step its lead hasn't merged
    /// (`unmerged`: a file of one hand-off, their changes laid in order, made by its agent with
    /// `state/pool/resume` on: agentcore::agent::resume) laid over the snapshot, as
    /// `Keys::load_with` lays the done records: a job resuming its own work (the OSM pass after
    /// sleep) finds its earlier stages' outputs (docs/pool.md §7.4). Its agent gives them only to a
    /// job that hands off (`SCENIC_HANDOFF`): one that saves the records has none. They're the snapshot's,
    /// not this job's changes: its saves hand off only its own.
    pub fn open_with(root: &store::nasfs::NasRoot, scratch: &Path, unmerged: Option<&Path>) -> Result<Self> {
        let handoff = std::env::var_os("SCENIC_HANDOFF").map(PathBuf::from);
        Self::open_from(root, scratch, unmerged, handoff, None)
    }

    /// `open_with`, the job handing off to `handoff` (`SCENIC_HANDOFF`), its manifest from the
    /// brief its agent wrote at `brief` (`BRIEF_ENV`) when it hands off: one NAS read fewer (the
    /// snapshot, ~14 MB), and never older than its lease.
    fn open_from(
        root: &store::nasfs::NasRoot,
        scratch: &Path,
        unmerged: Option<&Path>,
        handoff: Option<PathBuf>,
        brief: Option<&Path>,
    ) -> Result<Self> {
        let _p = jobrt::timings::phase("reading records");
        std::fs::create_dir_all(scratch)?;
        let manifest_path = root.join("state/build/manifest.json");
        let records: crate::records::Recorded = match (&handoff, brief) {
            (Some(_), Some(b)) => crate::records::from_brief(b)?,
            _ => crate::records::now(root)?,
        };
        let mut manifest = records.manifest;
        if let Some(p) = unmerged {
            let b = std::fs::read(p).with_context(|| format!("read this Mac's unmerged hand-offs, {}", p.display()))?;
            let h: crate::handoff::Handoff =
                serde_json::from_slice(&b).with_context(|| format!("parse {}", p.display()))?;
            lay_over(&mut manifest, &h);
        }
        // (A helper's job hands off only its own uploads.)
        let pending = if handoff.is_some() { Default::default() } else { records.pending };
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
            reused: Default::default(),
        })
    }

    /// Binds this to a job of `step` (scenic-build, as it starts a step): from now on it puts and
    /// removes only `step`'s outputs (`writes`: the function the lead's merge checks every
    /// entry against, so the writer and the check can't disagree; docs/pool.md §7.3).
    /// A put outside them fails before anything is uploaded; a `set` or `remove` outside them fails
    /// the next save, and nothing of that save lands (what earlier saves saved, the targets done
    /// before it, stands). A step the steps table doesn't know (a tool run by
    /// hand) is held to nothing while it writes the manifest, and changes nothing when it hands off
    /// (the merge refuses such an entry).
    /// A job's that hands off to `handoff`, as `SCENIC_HANDOFF` makes it (tests: the environment is
    /// the whole process's).
    #[cfg(any(test, feature = "testkit"))]
    pub fn handing_off(mut self, handoff: &Path) -> Self {
        self.handoff = Some(handoff.to_path_buf());
        self.pending = Default::default();
        self
    }

    fn for_step(&mut self, step: &str, writes: Writes) {
        self.step = Some((step.to_string(), writes));
    }

    /// Whether this job may put (`removal`: remove) `logical`; why not.
    fn admits(&self, logical: &str, removal: bool) -> Result<()> {
        let Some((step, writes)) = &self.step else { return Ok(()) };
        match writes(step, logical, removal) {
            Some(true) => Ok(()),
            Some(false) => bail!(
                "{} {logical}, outside {step}'s outputs (pipeline::agent::steps)",
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
    /// through `inputs::inputs::open` alone (docs/inputs.md §7.1), so a reader missed fails its
    /// first run instead of bypassing the gate.
    #[track_caller]
    pub fn path(&self, content: &str) -> NasPath {
        if crate::inputs_folder::in_inputs_folder(content) {
            panic!("{}: {content} is in an inputs folder; a step reads an input through inputs::inputs::open alone (docs/inputs.md §7.1)", std::panic::Location::caller());
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
    /// content name. An identical file already on the NAS is reused (`reused`), not copied again;
    /// one of the same name with another size is an error (content-named files are never rewritten).
    pub fn put_file(&mut self, logical: &str, ext: &str, local: &Path) -> Result<String> {
        if logical.contains('.') {
            bail!("logical names have no dots: {logical}");
        }
        self.admits(logical, false)?;
        self.settle_name(logical)?;
        let size = std::fs::metadata(local)?.len();
        let h = store::naming::hash16_file(local)?;
        let name = store::naming::content_name(logical, &h, ext);
        if self.uploaded.get(logical) == Some(&name) {
            // Uploaded under it by this very run already: nothing to look at again.
            std::fs::remove_file(local).ok();
            return Ok(self.landed(Landed {
                logical: logical.to_string(),
                name,
                pending: false,
                copied: false,
                size,
                ms: 0,
            }));
        }
        if self.manifest.get(logical) == Some(&name) {
            // The records name this very file under it: one look (task #449: heritage-sites'
            // slices, hundreds of small files nearly all unchanged). There at its size: taken.
            // Else `upload` as for any file: missing, put again; of another size, the put fails (a
            // damaged content-named file is removed by hand: store::naming).
            let dest = self.root.join(&name);
            if dest.metadata().is_ok_and(|m| m.len() == size) {
                std::fs::remove_file(local).ok();
                return Ok(self.landed(Landed {
                    logical: logical.to_string(),
                    name,
                    pending: true,
                    copied: false,
                    size,
                    ms: 0,
                }));
            }
        }
        let landed = upload_hashed(&self.root, logical, ext, local, &h, size)?;
        self.uploaded.insert(landed.logical.clone(), landed.name.clone());
        if landed.copied {
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
        let made = if l.copied { l.size } else { 0 };
        jobrt::timings::count(made, 1);
        if l.pending {
            if !l.copied {
                self.reused.insert(l.name.clone());
            }
            self.pending.insert(l.name.clone());
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
            let moved = self.later.moved.clone();
            let spawned = std::thread::Builder::new().name("uploads".into()).spawn(move || {
                store::naming::count_here_into(moved);
                for u in rx {
                    if tx.send(upload_retrying(&u.root, &u.logical, &u.ext, &u.local, upload_waits(testing()))).is_err()
                    {
                        return;
                    }
                }
            });
            if spawned.is_err() {
                // (No thread could be made: uploaded now.)
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

    /// Waits for the uploads put off up to `upto` to land, as long as their thread moves bytes
    /// (`UPLOAD_STALL`), and takes them into the records. The first that failed, or moved nothing
    /// for `UPLOAD_STALL`, fails this (and the job: what it put off isn't named).
    fn settle(&mut self, upto: Mark) -> Result<()> {
        if let Some(f) = &self.later.failed {
            bail!("an upload put off failed earlier: {f}");
        }
        if self.later.landed >= upto.0 {
            return Ok(());
        }
        let _p = jobrt::timings::phase("waiting for the uploads put off to land");
        let clock = self.later.clock.clone();
        let counted = self.later.moved.clone();
        let moved = || counted.load(std::sync::atomic::Ordering::Relaxed);
        // (The bytes moved when last seen to change, and when.)
        let mut seen = (moved(), clock.now());
        while self.later.landed < upto.0 {
            let got = loop {
                match self.later.back.as_ref().expect("an upload sent").lock().unwrap().try_recv() {
                    Ok(r) => break Some(r),
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => break None,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                }
                let (m, now) = (moved(), clock.now());
                if m != seen.0 {
                    seen = (m, now);
                } else if now.duration_since(seen.1) >= UPLOAD_STALL {
                    let size = self.later.sizes.front().map_or(0, |s| s.0);
                    let e = anyhow::anyhow!(
                        "an upload put off ({size} bytes) moved no byte in {} s (the NAS's share stuck?)",
                        UPLOAD_STALL.as_secs()
                    );
                    self.later.failed = Some(format!("{e:#}"));
                    return Err(e);
                }
                clock.sleep(UPLOAD_LOOK);
            };
            let l = match got {
                Some(Ok(l)) => l,
                Some(Err(e)) => {
                    let e = e.context("an upload put off failed");
                    self.later.failed = Some(format!("{e:#}"));
                    return Err(e);
                }
                None => {
                    self.later.failed = Some("the uploads' thread is gone".into());
                    bail!("the uploads' thread is gone");
                }
            };
            // (One landed is progress too.)
            seen = (moved(), clock.now());
            self.later.sizes.pop_front();
            let seq = self.later.landed;
            self.later.landed += 1;
            if l.copied {
                // (Its NAS file counted made as it lands, its bytes as store::naming wrote them;
                // its cost line's as its target claims it.)
                self.root.join(&l.name).count_made(l.size, l.ms);
                self.later.unclaimed.push((seq, l.size));
            }
            self.uploaded.insert(l.logical.clone(), l.name.clone());
            self.landed(l);
        }
        Ok(())
    }

    /// `put_file`, saying how far it is to `on` about once a second, as (bytes, total): it reads the
    /// file twice (its name's hash; the copy) and the copy once more on the NAS, for
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
    /// checked copies, which its index names: inputs::inputs::gate), and returns the content name.
    /// One there already is reused (`reused`), as `put_file` does.
    pub fn store_bytes(&mut self, logical: &str, ext: &str, bytes: &[u8]) -> Result<String> {
        let h = store::naming::hash16(bytes);
        let name = store::naming::content_name(logical, &h, ext);
        let dest = self.root.join(&name);
        // (There before: reused, as `upload_hashed` takes it.)
        let existed = dest.exists();
        let local = self.scratch_file(&format!("{logical}.{ext}"));
        std::fs::write(&local, bytes)?;
        let t = std::time::Instant::now();
        let got = store::naming::write_atomic_hashed(&self.root, logical, ext, &local, &h, bytes.len() as u64);
        std::fs::remove_file(&local).ok();
        anyhow::ensure!(got? == name, "{logical}: wrote another name than {name}");
        jobrt::timings::count(if existed { 0 } else { bytes.len() as u64 }, 1);
        if !existed {
            crate::costin::wrote(&dest, bytes.len() as u64, t.elapsed().as_millis() as u64);
        }
        self.sizes.insert(name.clone(), bytes.len() as u64);
        if existed {
            self.reused.insert(name.clone());
        }
        // (Its check pending whether copied or found there: a reuse is checked as an upload is.)
        self.pending.insert(name.clone());
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
    /// would: a removed step's file GC drops (agentcore::agent::gc::drop_names), never verified now.
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
        let _p = jobrt::timings::phase("saving records");
        // (Nothing saved of a job that named something outside its step's outputs.)
        if let Some(first) = self.outside.first() {
            bail!(
                "{first}{}: nothing saved",
                if self.outside.len() > 1 { format!(" (and {} more)", self.outside.len() - 1) } else { String::new() }
            );
        }
        if let Some(dir) = &self.handoff {
            if self.changes.is_empty() && self.pending.is_empty() && self.checked.is_empty() && self.reused.is_empty() {
                return Ok(());
            }
            let h = crate::handoff::Handoff {
                changes: self.changes.clone(),
                pending: self.pending.clone(),
                checked: self.checked.iter().cloned().collect(),
                sizes: self.named_sizes(),
                done: None,
                raw: Vec::new(),
                reused: self.reused.iter().cloned().collect(),
            };
            crate::handoff::write(dir, &h)?;
            self.sizes.retain(|c, _| !h.sizes.contains_key(c));
            self.changes.clear();
            self.pending.clear();
            self.checked.clear();
            self.reused.clear();
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
        self.pending.extend(h.pending.iter().cloned());
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
        let pending_read: crate::pending::Pending = read_record(&pending_path)?;
        let mut pending = pending_read.clone();
        pending.extend(self.pending.iter().cloned());
        pending.retain(|k| !self.checked.contains(k));
        if on_disk != read || !self.manifest_path.exists() {
            self.manifest_path.write_whole(serde_json::to_vec_pretty(&on_disk)?)?;
        }
        if pending != pending_read || !pending_path.exists() {
            pending_path.write_whole(serde_json::to_vec_pretty(&pending)?)?;
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

    /// The sizes the records hold of the files the manifest names (crate::records::now), with this run's own:
    /// what a catalogue lists them as, never looking at the NAS for them. An error when the records
    /// can't be read now; a file it lacks (written before sizes were recorded) isn't in it.
    pub fn recorded_sizes(&self) -> Result<BTreeMap<String, u64>> {
        let mut s = crate::records::now::<crate::records::Recorded>(&self.root)?.sizes;
        s.extend(self.sizes.iter().map(|(c, n)| (c.clone(), *n)));
        Ok(s)
    }
}

/// How long one call on the NAS (`verify`) may take: three minutes (the call's own
/// authentication, and a NAS busy with others' work), in a test too; a test of a hung call gives
/// its own thread's a short one (`short_nas_calls`). Its child's start is inside it, which on a
/// loaded test Mac took 25 s for a `/bin/sh` (#1157), so no test whose stand-in answers has a
/// short one.
pub fn nas_call_deadline() -> std::time::Duration {
    short_calls().unwrap_or(std::time::Duration::from_secs(180))
}

/// How long a call past its deadline has to end after SIGTERM before it's killed: 10 s; under a
/// test's short deadline (`short_nas_calls`), 300 ms.
fn nas_call_grace() -> std::time::Duration {
    if short_calls().is_some() {
        std::time::Duration::from_millis(300)
    } else {
        std::time::Duration::from_secs(10)
    }
}

/// This thread's short deadline (`short_nas_calls`): never one in a build without the testkit.
fn short_calls() -> Option<std::time::Duration> {
    #[cfg(feature = "testkit")]
    return SHORT_CALLS.get();
    #[cfg(not(feature = "testkit"))]
    None
}

#[cfg(feature = "testkit")]
std::thread_local! {
    static SHORT_CALLS: std::cell::Cell<Option<std::time::Duration>> = const { std::cell::Cell::new(None) };
}

/// A test's own deadline `d` for this thread's calls on the NAS (`nas_call_deadline`), for a test
/// of a call that hangs: the real one until the guard is dropped. Unit tests only: the testkit
/// feature, and `testing` (the feature reaches another crate's test-built binaries too).
#[cfg(feature = "testkit")]
pub fn short_nas_calls(d: std::time::Duration) -> ShortNasCalls {
    assert!(testing(), "a short NAS call deadline is a unit test's");
    SHORT_CALLS.set(Some(d));
    ShortNasCalls(())
}

/// `short_nas_calls`' guard: the real deadline again once dropped.
#[cfg(feature = "testkit")]
pub struct ShortNasCalls(());

#[cfg(feature = "testkit")]
impl Drop for ShortNasCalls {
    fn drop(&mut self) {
        SHORT_CALLS.set(None);
    }
}

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
    jobrt::sys::own_group(cmd);
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
            jobrt::sys::signal_group(pgid, jobrt::sys::Signal::Term);
        } else if stopped && !killed && t.elapsed() > deadline + grace {
            killed = true;
            jobrt::sys::signal_group(pgid, jobrt::sys::Signal::Kill);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let (stdout, stderr) = (o.join().unwrap_or_default(), e.join().unwrap_or_default());
    if stopped {
        return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "stopped past its deadline"));
    }
    Ok(std::process::Output { status, stdout, stderr })
}

/// The most bytes of quoted names one `verify` call sends (`sh_quoted`, joined by spaces): the shell
/// on the NAS takes the whole command as one argument, which Linux holds to 128 KB
/// (MAX_ARG_STRLEN), and the folder and the rest of the command fit in what's left.
pub const NAS_NAMES_MAX: usize = 96 * 1024;

/// `s` quoted for the NAS's shell: in single quotes, each of its own as `'\''`.
pub fn sh_quoted(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Checks unverified uploads `uploads` (content name → its size by the records; None where they hold
/// none) on the NAS itself, by one call over SSH (`ssh` runs a command on the NAS; `nas_root` is the
/// project folder as the NAS sees it): each file's size as the NAS has it (`wc -c`, which takes a
/// regular file's from its metadata and reads its last block at most), no SMB client's cache between.
/// The names there at their size (any size, for one the records hold none of), and those of another
/// size or not there; nothing is removed or written (GC's job takes the first out of the records'
/// pending: agentcore::agent::gc::verify_pending). Its bytes were hashed as they landed, read back
/// past this Mac's cache (`store::naming::write_atomic`), and the NAS's file system checksums them as
/// it reads them; what is left to check is that the file is still there, whole.
pub fn verify(
    ssh: &[&str],
    nas_root: &str,
    uploads: &BTreeMap<String, Option<u64>>,
) -> Result<(Vec<String>, Vec<String>)> {
    if uploads.is_empty() {
        return Ok(Default::default());
    }
    // (A first call, bounded: it authenticates, which may wait on the SSH agent unbounded
    // (crate::nasssh::SSH_START), and opens the shared connection the check's call then takes.)
    crate::nasssh::ssh_opened(ssh)?;
    let mut cmd = store::spawn::command(ssh[0]);
    cmd.args(&ssh[1..]);
    let quoted: Vec<String> = uploads.keys().map(|n| sh_quoted(n)).collect();
    let names = quoted.join(" ");
    anyhow::ensure!(
        names.len() <= NAS_NAMES_MAX,
        "{} uploads' names are {} bytes, over the {NAS_NAMES_MAX} one call takes",
        uploads.len(),
        names.len()
    );
    // (A file not there: wc says so on stderr and goes on; it's among those that don't match.)
    cmd.arg(format!("cd '{nas_root}' && {{ wc -c -- {names} 2>/dev/null; true; }}"));
    // (The call itself bounded too: its authentication, outside the probe's, may stall on the SSH
    // agent again, and the NAS may hang; past `nas_call_deadline` it is stopped.)
    let deadline = nas_call_deadline();
    let out = output_within(&mut cmd, deadline, nas_call_grace()).map_err(|e| {
        if e.kind() == std::io::ErrorKind::TimedOut {
            anyhow::Error::new(e).context(format!(
                "ssh to the NAS checking {} uploads: over {} s",
                uploads.len(),
                deadline.as_secs()
            ))
        } else {
            anyhow::Error::new(e).context("ssh to the NAS")
        }
    })?;
    anyhow::ensure!(out.status.success(), "ssh to the NAS: {}", String::from_utf8_lossy(&out.stderr).trim());
    let text = String::from_utf8_lossy(&out.stdout);
    // (Each line its size, a space and its name, the size padded on the left; a last `total` line,
    // which names no upload.)
    let got: BTreeMap<&str, u64> = text
        .lines()
        .filter_map(|l| l.trim_start().split_once(' '))
        .filter_map(|(n, name)| Some((name, n.parse().ok()?)))
        .collect();
    let (ok, bad): (Vec<_>, Vec<_>) =
        uploads.iter().partition(|(n, want)| got.get(n.as_str()).is_some_and(|g| want.is_none_or(|w| w == *g)));
    Ok((ok.into_iter().map(|(n, _)| n.clone()).collect(), bad.into_iter().map(|(n, _)| n.clone()).collect()))
}

/// Uploads local file `local` under `logical` to `root` (`Out::put_file`), the local file removed
/// after: its content name, its check pending. An identical file already on the NAS is reused, not
/// copied again; one of the same name with another size is an error (content-named files are never
/// rewritten).
fn upload(root: &store::nasfs::NasRoot, logical: &str, ext: &str, local: &Path) -> Result<Landed> {
    // (Its name's hash from one read of it: `write_atomic_hashed` checks the read-back against it.)
    let size = std::fs::metadata(local)?.len();
    upload_hashed(root, logical, ext, local, &store::naming::hash16_file(local)?, size)
}

/// `upload` of a file whose `hash16` and size are known.
fn upload_hashed(
    root: &store::nasfs::NasRoot,
    logical: &str,
    ext: &str,
    local: &Path,
    h: &str,
    size: u64,
) -> Result<Landed> {
    let name = store::naming::content_name(logical, h, ext);
    let dest = root.join(&name);
    // (There before: reused, nothing written to it, its check pending as an upload's. Removed
    // between the look and the write, it's written afresh and still taken as reused: the merge
    // refuses the hand-off, GC having removed it under its lease.)
    let existed = dest.exists();
    let t = std::time::Instant::now();
    let got = store::naming::write_atomic_hashed(root, logical, ext, local, h, size)?;
    anyhow::ensure!(got == name, "{logical}: wrote {got}, expected {name}");
    std::fs::remove_file(local).ok();
    let ms = t.elapsed().as_millis() as u64;
    Ok(Landed { logical: logical.to_string(), name, pending: true, copied: !existed, size, ms })
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

/// An error once the pool has begun (a lead declared, crate::records::lead::LEAD): the records are
/// its lead's to write, through its merges alone, and every job hands its saves off
/// (SCENIC_HANDOFF, docs/pool.md §7.3), whatever Mac it runs on. A folder with no lead declared (a
/// test's, a scratch build's) has its records written as today's files (the pool's first take-up
/// starts from them).
pub fn check_writer(root: &store::nasfs::NasRoot) -> Result<()> {
    let lead = root.join(crate::records::lead::LEAD);
    if lead.exists() {
        bail!("the pool leads this build ({} exists): the records are its lead's to write; a step saves only as a job's, handing off (SCENIC_HANDOFF)", lead.display());
    }
    Ok(())
}

/// An exclusive lock for saving a root's manifest among this Mac's processes, in a local file named
/// after the root: released when dropped.
pub struct BuildLock(#[allow(dead_code)] jobrt::sys::PathLock);

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
        let l = jobrt::sys::PathLock::take(&Self::file(root), true).context("lock the build manifest")?;
        Ok(BuildLock(l.context("lock the build manifest: not taken")?))
    }

    /// The lock when it's free now; None when another holds it (a paused job may, for hours).
    pub fn try_take(root: &store::nasfs::NasRoot) -> Result<Option<BuildLock>> {
        Ok(jobrt::sys::PathLock::take(&Self::file(root), false).context("lock the build manifest")?.map(BuildLock))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tests' write sets: two steps' outputs as pipeline::agent::steps has them (`pack`'s map
    /// tiles' data and road and rail packs at z6; the OSM pass's sources); any other step unknown.
    fn writes(step: &str, logical: &str, _: bool) -> Option<bool> {
        let z6 = |prefix: &str| {
            logical.strip_prefix(prefix).is_some_and(|t| {
                let p: Vec<&str> = t.split('-').collect();
                p.len() == 3 && p[0] == "6" && p.iter().all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            })
        };
        match step {
            "pack" => Some(z6("hidata/") || z6("layers/roads/hi/") || z6("layers/rails/hi/")),
            "osm-pass" => Some(logical.starts_with("sources/osm/")),
            _ => None,
        }
    }

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
            let p: crate::pending::Pending = read_record(&root.join("state/build/pending.json")).unwrap();
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
        let mut job =
            Out::open(&store::nasfs::NasRoot::local_for_test(&later), &d.path().join("s3"), "pack", writes).unwrap();
        assert!(job.put_file_later("base/6-1-5", "base", &file("b5", b"five")).is_err());
    }

    /// A job whose uploads put off are a test's (`tx`: their results; `moved`: the bytes their
    /// thread moved), waited on by `clock`: one sent.
    fn fake_uploads(
        d: &Path,
        clock: Arc<store::clock::Virtual>,
    ) -> (Out, std::sync::mpsc::Sender<Result<Landed>>, Arc<std::sync::atomic::AtomicU64>) {
        let mut c =
            Out::open(&store::nasfs::NasRoot::local_for_test(&d.join("nas")), &d.join("s"), "pack", writes).unwrap();
        let (tx, back) = std::sync::mpsc::channel();
        c.later.back = Some(std::sync::Mutex::new(back));
        c.later.clock = clock;
        c.later.sent = 1;
        c.later.sizes.push_back((400 << 20, "base/6-1-1".into()));
        let moved = c.later.moved.clone();
        (c, tx, moved)
    }

    use std::sync::Arc;

    /// Task #911: an upload on a busy share, slow but moving, lands however long it takes (here
    /// 9 min for 400 MB, a byte every 100 s: far under the old 1.6 MB/s floor).
    #[test]
    fn a_slow_but_moving_upload_lands() {
        let d = tempfile::tempdir().unwrap();
        let clock = store::clock::Virtual::new();
        let (mut c, tx, moved) = fake_uploads(d.path(), clock.clone());
        for k in 1..=5 {
            let m = moved.clone();
            clock.at(UPLOAD_STALL * k * 5 / 6, move || {
                m.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            });
        }
        let landed = Landed {
            logical: "base/6-1-1".into(),
            name: "base/6-1-1.0123456789abcdef.base".into(),
            pending: false,
            copied: true,
            size: 400 << 20,
            ms: 0,
        };
        clock.at(UPLOAD_STALL * 9 / 2, move || tx.send(Ok(landed)).unwrap());
        c.settle(c.mark()).unwrap();
        assert!(clock.elapsed() >= UPLOAD_STALL * 9 / 2);
        assert_eq!(c.get("base/6-1-1"), Some("base/6-1-1.0123456789abcdef.base"));
    }

    /// Task #911: an upload that stops moving fails once `UPLOAD_STALL` passes with no byte moved
    /// (counted from the last), and every later save with it.
    #[test]
    fn a_stalled_upload_fails_within_the_stall_window() {
        let d = tempfile::tempdir().unwrap();
        let clock = store::clock::Virtual::new();
        let (mut c, _tx, moved) = fake_uploads(d.path(), clock.clone());
        let last = UPLOAD_STALL / 2;
        clock.at(last, move || {
            moved.fetch_add(1 << 20, std::sync::atomic::Ordering::Relaxed);
        });
        let e = c.settle(c.mark()).unwrap_err();
        assert!(format!("{e:#}").contains(&format!("moved no byte in {} s", UPLOAD_STALL.as_secs())), "{e:#}");
        let t = clock.elapsed();
        assert!(t >= last + UPLOAD_STALL && t <= last + UPLOAD_STALL + UPLOAD_LOOK, "{t:?}");
        assert!(c.save().is_err());
    }

    /// Task #449: a file the records already name under its logical name is looked at once: there
    /// at its size, taken (reused, not written to); missing or of another size, put back; one this
    /// run landed, not looked at again. Other bytes, or another name, are uploaded as before.
    #[test]
    fn a_file_the_records_name_is_checked_once_and_put_back_if_missing() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        let l = "work/heritage/d/pos/6-1-1";
        let mut out = Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("scratch")).unwrap();
        let a = out.put_bytes(l, "json", b"[[1,2]]").unwrap();
        out.save().unwrap();
        let open = || Out::unbound(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("scratch")).unwrap();
        // There: taken as it is (no touch: GC goes by the queue, docs/storage.md, GC).
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86400);
        std::fs::File::options().write(true).open(root.join(&a)).unwrap().set_modified(old).unwrap();
        let mut out = open();
        assert_eq!(out.put_bytes(l, "json", b"[[1,2]]").unwrap(), a);
        assert!(
            std::fs::metadata(root.join(&a)).unwrap().modified().unwrap() <= old + std::time::Duration::from_secs(1),
            "not touched"
        );
        assert!(out.reused.contains(&a) && out.pending.contains(&a), "its check pending as an upload's");
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
        out.for_step("osm-pass", writes);
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

    /// docs/coordination.md step 8d: a job that hands off opens its manifest from the brief its
    /// agent wrote (`BRIEF_ENV`), reading no records on the NAS (a pool's root with none readable
    /// there: an error for a reader of the NAS); one that doesn't hand off reads the NAS as before,
    /// brief or not; a brief that can't be read fails the job rather than open other records.
    #[test]
    fn a_job_handing_off_opens_from_its_agents_brief() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        std::fs::write(root.join(crate::records::lead::LEAD), b"{}").unwrap();
        let nas = store::nasfs::NasRoot::local_for_test(&root);
        let brief = d.path().join("brief.json");
        std::fs::write(
            &brief,
            r#"{"term":849,"seq":3,"manifest":{"base/6-1-1":"base/6-1-1.1111111111111111.base"},"keys":{}}"#,
        )
        .unwrap();
        let (scratch, hand) = (d.path().join("scratch"), Some(d.path().join("handoff")));
        let out = Out::open_from(&nas, &scratch, None, hand.clone(), Some(&brief)).unwrap();
        assert_eq!(out.get("base/6-1-1"), Some("base/6-1-1.1111111111111111.base"));
        assert!(out.pending.is_empty() && out.handoff.is_some());
        assert!(Out::open_from(&nas, &scratch, None, None, Some(&brief)).is_err(), "the NAS's, unreadable");
        assert!(Out::open_from(&nas, &scratch, None, hand.clone(), None).is_err(), "no brief: the NAS's");
        assert!(Out::open_from(&nas, &scratch, None, hand, Some(&d.path().join("gone.json"))).is_err());
    }

    /// Task #1002: an upload reads its file for its name's hash alone (`hash16_file`: no other
    /// hash beside it), and its check pending is handed off by its name, an empty string where
    /// apps before it wrote its SHA-256 (crate::pending). A file the records name already, there
    /// at its size, is read once, for that hash, and nothing else (across chunks: a file larger
    /// than one, its last one short).
    #[test]
    fn an_upload_reads_its_file_for_its_name_alone_and_hands_off_no_checksum() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        let nas = store::nasfs::NasRoot::local_for_test(&root);
        let mut out = Out::unbound(&nas, &d.path().join("scratch")).unwrap();
        let bytes: Vec<u8> = (0..(store::naming::CHUNK * 2 + 12345)).map(|i| (i * 31 % 251) as u8).collect();
        let local = |n: &str| {
            let p = d.path().join(n);
            std::fs::write(&p, &bytes).unwrap();
            p
        };
        let name = out.put_file("work/x/6-1-1", "bin", &local("a")).unwrap();
        assert_eq!(name, store::naming::content_name("work/x/6-1-1", &store::naming::hash16(&bytes), "bin"));
        assert!(out.pending.contains(&name));
        // (This thread's reads alone: the one look a named file takes.)
        let moved = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        store::naming::count_here_into(moved.clone());
        assert_eq!(out.put_file("work/x/6-1-1", "bin", &local("b")).unwrap(), name);
        assert_eq!(moved.load(std::sync::atomic::Ordering::Relaxed), bytes.len() as u64, "read once, for its name");
        let hd = d.path().join("handoffs");
        out.handoff = Some(hd.clone());
        out.save().unwrap();
        let b = std::fs::read(std::fs::read_dir(&hd).unwrap().next().unwrap().unwrap().path()).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&b).unwrap();
        assert_eq!(v["pending"], serde_json::json!({ name.clone(): "" }));
        // (What an app before task #1002 reads a hand-off's pending as: a map of strings.)
        let theirs: BTreeMap<String, String> = serde_json::from_value(v["pending"].clone()).unwrap();
        assert_eq!(theirs.into_keys().collect::<Vec<_>>(), [name]);
    }

    /// A file there already is reused as it is, its time left (GC goes by its queue, not by
    /// files' times: docs/storage.md, GC), and handed off as `reused`, which the merge checks
    /// against GC's deletions; its check is pending as an upload's (the uploads' verification finds
    /// one missing, as it does an upload).
    #[test]
    fn a_file_reused_is_handed_off_as_reused_untouched() {
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
        out.reused.clear();
        assert_eq!(out.put_file("work/x/6-1-1", "bin", &local("b")).unwrap(), name);
        assert!(
            std::fs::metadata(&dest).unwrap().modified().unwrap() <= old + std::time::Duration::from_secs(1),
            "not touched"
        );
        assert!(out.reused.contains(&name) && out.pending.contains(&name), "reused, its check pending");
        // The gate's copies (`store_bytes`) likewise.
        let copy = out.store_bytes("sources/inputs/_gate-test/a", "jsonl", b"the same lines").unwrap();
        out.pending.clear();
        let cdest = root.join(&copy);
        std::fs::File::options().write(true).open(&cdest).unwrap().set_modified(old).unwrap();
        assert_eq!(out.store_bytes("sources/inputs/_gate-test/a", "jsonl", b"the same lines").unwrap(), copy);
        assert!(
            std::fs::metadata(&cdest).unwrap().modified().unwrap() <= old + std::time::Duration::from_secs(1),
            "not touched"
        );
        assert!(out.pending.contains(&copy) && out.reused.contains(&copy));
        // Gone (GC took it): written again, there, its check pending.
        std::fs::remove_file(&cdest).unwrap();
        out.reused.clear();
        assert_eq!(out.store_bytes("sources/inputs/_gate-test/a", "jsonl", b"the same lines").unwrap(), copy);
        assert!(cdest.exists() && out.pending.contains(&copy) && !out.reused.contains(&copy));
        // A helper's save hands the reused names off with its changes.
        let hd = d.path().join("handoffs");
        out.handoff = Some(hd.clone());
        out.reused.insert(name.clone());
        out.save().unwrap();
        let h: crate::handoff::Handoff = serde_json::from_slice(
            &std::fs::read(std::fs::read_dir(&hd).unwrap().next().unwrap().unwrap().path()).unwrap(),
        )
        .unwrap();
        assert_eq!(h.reused, [name.clone()]);
        assert!(out.reused.is_empty());
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
        // jobrt::sys::PathLock.)
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
        // No lead declared: saved, as today's files.
        out.changes.insert("a".into(), Some("a.1111111111111111.x".into()));
        out.save().unwrap();
        assert!(root.join("state/build/manifest.json").exists());
        // The pool begun (a lead declared): no one's step writes them (a job hands off).
        std::fs::write(root.join(crate::records::lead::LEAD), b"{}").unwrap();
        out.changes.insert("b".into(), Some("b.2222222222222222.x".into()));
        assert!(format!("{:#}", out.save().unwrap_err()).contains("the pool leads this build"));
        assert!(check_writer(&store::nasfs::NasRoot::local_for_test(&root)).is_err());
    }

    /// A job puts and removes only its step's outputs (`writes`, the function the lead's
    /// merge checks by): a put outside fails before it uploads, a set or removal outside
    /// fails the save before anything is saved, and a step the table doesn't know hands off nothing.
    #[test]
    fn a_job_writes_only_its_steps_outputs() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("root");
        let local = d.path().join("f.bin");
        let mut out =
            Out::open(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("s"), "pack", writes).unwrap();
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
            Out::open(&store::nasfs::NasRoot::local_for_test(&root), &d.path().join("s2"), "patch-ferries", writes)
                .unwrap();
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
