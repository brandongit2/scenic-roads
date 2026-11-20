//! A long target's run resumed where it stopped (docs/pool.md §7.2, Resuming): its work goes
//! block by block, and a run stopped (sleep, an update, the NAS restarting, the memory guard, a
//! pause) starts again at the block after the last it finished. The steps keeping one: depth and
//! depth-lo (crate::depth::resume), terrain-hi (crate::terrain_pack, a level's blocks),
//! terrain-deep (crate::terrain_deep, its base tiles), and through `Pieces` (task #889) unit,
//! pois, peaks, bldtiles, bldprep, bldfield-lo, trees, polar-terrain, polar-slope, slope-lo and lo;
//! (task #890) polar-root, reach and water; (task #897) regions-extract, names-todo and the
//! regions' input check; (task #1414) slope-deep and lake-depth.
//!
//! - **On this Mac's disk while it runs:** one file per target at the top of the job's scratch
//!   folder (`<scratch>/<z>_<x>_<y>.resume`, `path`; each step's scratch folder is its own): a line
//!   naming the step's kind and the run's identity (`id`), then a log appended once a block (its
//!   block and what the step keeps of it, each record with its length and BLAKE3, so a record cut
//!   short or damaged is dropped with what follows it). At the top, the agent's sampler counts it in
//!   the job's disk (agentcore::agent::memguard's `top_bytes`), so the step's measured disk, and the room
//!   asked for it, hold it.
//! - **Read a record at a time:** a log is streamed as it opens (`Log::open`'s `take`), never read
//!   whole; `Pieces` notes only where each record lies and reads one as its piece is taken (task
//!   #1182), so a resumed run holds what its step holds of the pieces it has taken, as the run it
//!   resumes held them.
//! - **Taken only for the same inputs:** the identity is a digest of what decides the output: the
//!   step's version(s), its target and everything the run reads, its step's choosing; never the
//!   program, so a resume in a newly installed app keeps what was kept. Another identity, the log
//!   is emptied and the run starts afresh.
//! - **Where it's found:** the job's scratch folder, or the same step's folder of another of the
//!   agent's slots (`scratch-2/`, …), moved here; or, given back on another Mac, carried on the NAS
//!   (task #1414): a job stopping at a safe point (`crate::control::stop_paused`, and any paused
//!   exit, `crate::control::exit`'s: a pause, an update's drain) copies the logs it grew to
//!   `CARRIED/<step>/<target>/`, its target as the plan names it, before it exits (`carry_out`),
//!   so before its agent sees its end and gives its lease back, the lease beating meanwhile; its
//!   bytes are the job's own, in its record. A job ended otherwise (a failure, a signal: a guard's
//!   or a yield's stop, a sleep, a cancel), or drained for memory, carries nothing. The next
//!   run of the target, on any Mac, takes that copy when it holds more of the run's identity than
//!   this Mac's own (`take_carried`; the copy deleted once looked at, the target this Mac's). A log
//!   is one file, so the NAS takes it as one stream. A log holding work kept beside it (unit's
//!   folders, `Pieces::folder_piece`) says so in its first line (`local_head`) and is never carried
//!   nor taken: such a step resumes on its own Mac alone.
//! - **Gone:** once its step's packs are written (each step deletes its own); when its job is
//!   cancelled (the agent clears the target's scratch, `<z>_<x>_<y>.*`); with the slot's step folder
//!   when room-making deletes a given-back job's; and, a target that never comes back here (made on
//!   another Mac), as the step's next run here opens its own log: any log in the step's folders
//!   untouched for `HOLD_S` (`sweep`). A carried copy, once a run looks at it; with its target's
//!   job cancelled (`drop_carried`); or once the lead's plan neither plans nor leases its target in
//!   its step (`sweep_carried`, which also deletes a copy cut short, its temporary file, a day on).
//! - **Only with resuming on:** the job's agent says so (`RESUME_ENV`, in the pool); a run without
//!   it keeps nothing.

use anyhow::Result;
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// A job's environment: resuming is on (the agent sets it in the pool: agentcore's agent::resume):
/// its blocks are kept here, and a pass is complete when its records say
/// so (crate::osmpass's `latest_pass`).
pub const RESUME_ENV: &str = "SCENIC_RESUME";

/// How long a job's progress is offered to its Mac alone, from when its job last started or ended
/// there (docs/pool.md §7.2: 24 hours; pipeline's agent::resume); a log untouched this long is swept.
pub const HOLD_S: u64 = 24 * 3600;

/// A block: (z, and two numbers its step names it by: its first tile's column and row, …).
pub type BlockId = (u8, u32, u32);

/// A job's environment: the NAS project folder a log is carried through (its agent's `--root`
/// for it, set with resuming on, for a job of a planned step): `carry_out`, `take_carried`.
pub const CARRY_ENV: &str = "SCENIC_RESUME_CARRY";

/// A job's environment beside `CARRY_ENV`: its step, as the plan names it (`CARRIED/<step>/`).
pub const CARRY_STEP_ENV: &str = "SCENIC_RESUME_CARRY_STEP";

/// A job's environment beside `CARRY_ENV`: its targets as the plan names them, a line each (a
/// log's copy is filed under its target's: `target_of`).
pub const CARRY_TARGETS_ENV: &str = "SCENIC_RESUME_CARRY_TARGETS";

/// A job's environment beside `CARRY_ENV`, when its agent knows it: the NAS's throughput as the
/// lead's probes last measured it (bytes a second), what a carry is weighed by when the job has
/// measured none of its own (`worth_carrying`).
pub const CARRY_RATE_ENV: &str = "SCENIC_RESUME_CARRY_RATE";

/// Where on the NAS a log given back waits for its target's next run:
/// `<CARRIED>/<step>/<target, its slashes %2F>/<log>`.
pub const CARRIED: &str = "state/pool/carried";

/// Where a job's logs are carried: the NAS root, its step and targets as the plan names them,
/// and the lead's probed NAS throughput (bytes a second) when its agent knew it.
#[derive(Clone, Debug)]
pub struct Carry {
    pub root: store::nasfs::NasRoot,
    pub step: String,
    pub targets: Vec<String>,
    pub probed: Option<f64>,
}

impl Carry {
    /// Tests: `root`, step `step`, targets `targets`, no probe.
    #[cfg(any(test, feature = "testkit"))]
    pub fn test(root: &store::nasfs::NasRoot, step: &str, targets: &[&str]) -> Carry {
        let targets = targets.iter().map(|t| t.to_string()).collect();
        Carry { root: root.clone(), step: step.into(), targets, probed: None }
    }

    /// Log tile `t`'s target among the job's: the one whose last three parts are its z, x and y
    /// (`6/33/22`, `n/4/3/1`), else the job's only target (a target not named by a tile: a pass's,
    /// a unit's check); None when neither names it.
    fn target_of(&self, t: (u8, u32, u32)) -> Option<&str> {
        let tile = |s: &str| {
            let p: Vec<&str> = s.rsplit('/').take(3).collect();
            p.len() == 3 && p[2].parse() == Ok(t.0) && p[1].parse() == Ok(t.1) && p[0].parse() == Ok(t.2)
        };
        match self.targets.iter().find(|s| tile(s)) {
            Some(s) => Some(s),
            None if self.targets.len() == 1 => Some(&self.targets[0]),
            None => None,
        }
    }
}

#[cfg(any(test, feature = "testkit"))]
thread_local! {
    /// Tests: where logs are carried on this thread, in place of the environment's.
    pub static CARRY_ROOT: std::cell::RefCell<Option<Carry>> = const { std::cell::RefCell::new(None) };
}

/// Where this job's logs are carried: its `CARRY_ENV`, `CARRY_STEP_ENV`, `CARRY_TARGETS_ENV` and
/// `CARRY_RATE_ENV` (a test's `CARRY_ROOT`); none for a run by hand.
fn carry_root() -> Option<Carry> {
    #[cfg(any(test, feature = "testkit"))]
    if let Some(r) = CARRY_ROOT.with(|c| c.borrow().clone()) {
        return Some(r);
    }
    static ROOT: std::sync::OnceLock<Option<Carry>> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        let root = std::env::var_os(CARRY_ENV)?;
        let step = std::env::var(CARRY_STEP_ENV).ok().filter(|s| !s.is_empty() && !s.contains(['/', '.']))?;
        let targets = std::env::var(CARRY_TARGETS_ENV).ok()?.lines().map(str::to_string).collect();
        let probed = std::env::var(CARRY_RATE_ENV).ok().and_then(|r| r.parse().ok()).filter(|r: &f64| *r > 0.0);
        Some(Carry { root: store::nasfs::NasRoot::from_arg(PathBuf::from(root)), step, targets, probed })
    })
    .clone()
}

/// A target as a folder's name: its `%` as `%25` and `/` as `%2F`.
fn folder_of(target: &str) -> String {
    target.replace('%', "%25").replace('/', "%2F")
}

/// `folder_of`'s target back.
fn target_of_folder(name: &str) -> String {
    name.replace("%2F", "/").replace("%25", "%")
}

/// Target `target`'s carried copies for step `step` under `root`: a folder.
fn carried_dir(root: &store::nasfs::NasRoot, step: &str, target: &str) -> store::nasfs::NasPath {
    root.join(format!("{CARRIED}/{step}/{}", folder_of(target)))
}

/// Log tile `t`'s carried copy, its target the job's `target_of`.
fn carried(c: &Carry, t: (u8, u32, u32)) -> Option<store::nasfs::NasPath> {
    let target = c.target_of(t)?;
    Some(carried_dir(&c.root, &c.step, target).join(format!("{}_{}_{}{EXT}", t.0, t.1, t.2)))
}

/// A log this process opened, as `carry_out` weighs it.
struct Opened {
    path: PathBuf,
    t: (u8, u32, u32),
    /// Its length as it opened (what an earlier run left), and as it was last carried: a log that
    /// grew no further holds nothing this run would lose.
    at_open: u64,
    opened: std::time::Instant,
    /// When this run first and last kept a block in it, and how many it kept; the blocks it holds
    /// in all.
    first_keep: Option<std::time::Instant>,
    last_keep: Option<std::time::Instant>,
    new: u64,
    total: u64,
    /// Its first line says it holds work kept beside it (`local_head`).
    local: bool,
    /// Tests: the thread that opened it (a test's job), the only one that carries it.
    #[cfg(any(test, feature = "testkit"))]
    thread: std::thread::ThreadId,
}

impl Opened {
    /// The work it holds (s): this run's time a block, kept after kept (from its first block, so
    /// not the run's setup, which a resumed run does again; one block alone, from the log's
    /// opening), times the blocks it holds.
    fn work_s(&self) -> f64 {
        let (Some(first), Some(last)) = (self.first_keep, self.last_keep) else { return 0.0 };
        let each = if self.new >= 2 {
            last.duration_since(first).as_secs_f64() / (self.new - 1) as f64
        } else {
            last.duration_since(self.opened).as_secs_f64()
        };
        each * self.total as f64
    }
}

/// The logs this process opened (`Log::open`), each once, as they stand.
static OPENED: std::sync::Mutex<Vec<Opened>> = std::sync::Mutex::new(Vec::new());

fn with_opened(f: impl FnOnce(&mut Vec<Opened>)) {
    f(&mut OPENED.lock().unwrap_or_else(|e| e.into_inner()));
}

/// Tests: the logs this thread opened forgotten, as a new job's process starts with none.
#[cfg(any(test, feature = "testkit"))]
pub fn forget_opened() {
    let me = std::thread::current().id();
    with_opened(|v| v.retain(|o| o.thread != me));
}

/// Whether a log holding work of `work_s` (`Opened::work_s`) is worth `bytes` carried at `rate`
/// (bytes a second: None when neither the job nor the lead's probes measured one): its upload and
/// the next run's download take less than the work they spare.
pub fn worth_carrying(bytes: u64, rate: Option<f64>, work_s: f64) -> bool {
    rate.filter(|r| *r > 0.0).is_none_or(|r| 2.0 * bytes as f64 / r <= work_s)
}

/// This job's NAS rate so far: the bytes it moved over the wall time it waited on the NAS
/// (store::waits; None before it waited a second, or with its waits unmeasured).
fn nas_rate() -> Option<f64> {
    use store::nasio::{get, Count};
    let waited = store::waits::totals()[store::waits::On::Nas as usize] as f64 / 1e9;
    let bytes = (get(Count::ReadBytes) + get(Count::WroteBytes)) as f64;
    (waited >= 1.0 && bytes > 0.0).then(|| bytes / waited)
}

/// The lock a Mac's carries take in turn, in the home of step folder `scratch`'s slots
/// (`<home>/scratch[-n]/<step>`: `<home>/carry.lock`), else in `scratch`.
fn mac_lock(scratch: &Path) -> PathBuf {
    let slot = scratch.parent().filter(|s| s.file_name().is_some_and(|n| n.to_string_lossy().starts_with("scratch")));
    match slot.and_then(Path::parent) {
        Some(home) => home.join("carry.lock"),
        None => scratch.join("carry.lock"),
    }
}

/// The Mac's carry lock (`mac_lock`), waited for at most `most`: past it, waiting would cost more
/// than the carry spares. None then, or when it can't be taken.
fn turn_within(lock: &Path, most: std::time::Duration) -> Option<crate::sys::PathLock> {
    let t = std::time::Instant::now();
    loop {
        match crate::sys::PathLock::take(lock, false) {
            Ok(Some(l)) => return Some(l),
            Ok(None) if t.elapsed() < most => std::thread::sleep(std::time::Duration::from_millis(250)),
            Ok(None) => return None,
            Err(e) => {
                eprintln!("resume: carrying the logs: {}: {e}", lock.display());
                return None;
            }
        }
    }
}

/// The job stops at a safe point, its targets not all made (crate::control::stop_paused, and any
/// paused exit: crate::control::exit): the logs it opened and grew, each copied to the NAS
/// (`CARRIED/<step>/<target>/`, replacing an older copy) for the target's next run wherever it
/// goes (`take_carried`), before the job exits, so before its agent gives its lease back, the lease
/// beating meanwhile and its agent told it's carrying (crate::control::carrying: not frozen for a
/// safe point it has reached). Its bytes counted as the job's (store::nasio, in its record).
/// Nothing when its agent drains it for memory (crate::control::no_carry: the memory it holds is
/// wanted now). Not a log that holds work kept beside it (`local_head`), nor one whose copy would
/// take longer up and down than the work it holds (`worth_carrying`, at the job's NAS rate, else
/// the lead's probed one), nor one no target of the job names (`Carry::target_of`). One Mac's
/// carries go one at a time (`mac_lock`), the turn waited for only while waiting and copying cost
/// less than the work the logs hold (`turn_within`); each a turn of the job's NAS limit
/// (store::nasfs's `copy_from_local_paced`). Each log copied and its bytes; one that fails is said
/// and left. Called again, only what grew since.
pub fn carry_out() -> Vec<(String, u64)> {
    let Some(c) = carry_root() else { return Vec::new() };
    let mut grew = Vec::new();
    with_opened(|v| {
        for o in v.iter().filter(|o| !o.local && o.new > 0) {
            #[cfg(any(test, feature = "testkit"))]
            if o.thread != std::thread::current().id() {
                continue;
            }
            let len = std::fs::metadata(&o.path).map_or(0, |m| m.len());
            if len > o.at_open {
                grew.push((o.path.clone(), o.t, len, o.work_s()));
            }
        }
    });
    let Some(scratch) = grew.first().and_then(|g| g.0.parent().map(Path::to_path_buf)) else { return Vec::new() };
    if crate::control::no_carry() {
        eprintln!("resume: {} logs not carried: drained for memory", grew.len());
        return Vec::new();
    }
    crate::control::carrying();
    let rate = nas_rate().or(c.probed);
    let mut v = Vec::new();
    let mut weighed = Vec::new();
    for (p, t, len, work_s) in grew {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let Some(to) = carried(&c, t) else {
            eprintln!("resume: {name} not carried: no target of the job's names it");
            continue;
        };
        if !worth_carrying(len, rate, work_s) {
            let mb_s = rate.unwrap_or_default() / 1e6;
            eprintln!("resume: {name} not carried: {len} bytes at {mb_s:.1} MB/s up and down take longer than its {work_s:.0} s of work");
            continue;
        }
        weighed.push((p, len, work_s, name, to));
    }
    let spare: f64 = weighed.iter().map(|w| w.2 - rate.map_or(0.0, |r| 2.0 * w.1 as f64 / r)).sum();
    let lock = mac_lock(&scratch);
    let Some(_turn) = turn_within(&lock, std::time::Duration::from_secs_f64(spare.max(0.0))) else {
        if !weighed.is_empty() {
            eprintln!(
                "resume: {} logs not carried: this Mac's carries took longer than the {spare:.0} s they'd spare",
                weighed.len()
            );
        }
        return v;
    };
    for (p, len, _, name, to) in weighed {
        let r = to
            .parent()
            .map_or(Ok(()), |d| d.create_dir_all())
            .map_err(anyhow::Error::from)
            .and_then(|()| to.copy_from_local_paced(&p));
        match r {
            Ok(n) => {
                eprintln!("resume: {name}: {} MB carried to the NAS for the target's next run", n >> 20);
                with_opened(|v| v.iter_mut().filter(|o| o.path == p).for_each(|o| o.at_open = len));
                v.push((name, n));
            }
            Err(e) => eprintln!("resume: {} not carried to the NAS: {e:#}", to.display()),
        }
    }
    v
}

/// Target `target`'s copies carried for step `step` (`carry_out`), deleted: its job cancelled
/// (agentcore's agent, cancel_slot), its work dropped. How many went.
pub fn drop_carried(root: &store::nasfs::NasRoot, step: &str, target: &str) -> usize {
    let d = carried_dir(root, step, target);
    let n = d.read_dir().into_iter().flatten().filter(|e| e.path.remove_file().is_ok()).count();
    d.remove_dir().ok();
    n
}

/// The lead's sweep of the carried copies (`CARRIED/<step>/<target>/`): a target's copies `keep`
/// (its step, its target as the plan names it) says neither the plan plans nor a lease holds,
/// deleted (made, re-keyed away or gone: no run will take them); and a copy cut short (its
/// temporary file, store::whole's `left_by_write`) left `store::whole::ASIDE_KEPT`, deleted. How
/// many went.
pub fn sweep_carried(root: &store::nasfs::NasRoot, keep: &dyn Fn(&str, &str) -> bool) -> usize {
    let now = std::time::SystemTime::now();
    let old = |e: &store::nasfs::NasEntry| {
        e.path
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .is_some_and(|t| now.duration_since(t).is_ok_and(|a| a >= store::whole::ASIDE_KEPT))
    };
    let mut n = 0;
    for s in root.join(CARRIED).read_dir().into_iter().flatten().filter(|e| e.is_dir()) {
        for d in s.path.read_dir().into_iter().flatten().filter(|e| e.is_dir()) {
            let kept = keep(&s.name, &target_of_folder(&d.name));
            for e in d.path.read_dir().into_iter().flatten() {
                let cut = store::whole::left_by_write(&e.name) && old(&e);
                if (!kept || cut) && e.path.remove_file().is_ok() {
                    eprintln!("resume: {} deleted: no run will take it", e.path.display());
                    n += 1;
                }
            }
            if !kept {
                d.path.remove_dir().ok();
            }
        }
    }
    n
}

/// Target `t`'s log `p`'s copy carried on the NAS (`carry_out`), taken in its place when it holds
/// more of the run's identity (its first line `head`; a longer log of one identity holds more, as
/// each is appended to from where the last run left it), then deleted from the NAS either way: the
/// target is this Mac's. The copy's first line read on the NAS first, so one of another identity
/// is never downloaded; and the copy taken weighed again against this Mac's own before it's put in
/// its place. A copy that can't be read is left for a later run.
fn take_carried(carry: Option<&Carry>, t: (u8, u32, u32), p: &Path, head: &str) {
    let Some(c) = carry.and_then(|c| carried(c, t)) else { return };
    let Ok(m) = c.metadata() else { return };
    let ours = |f: &Path| -> u64 {
        let mut first = vec![0u8; head.len()];
        let same = std::fs::File::open(f).and_then(|mut r| r.read_exact(&mut first)).is_ok() && is_head(&first, head);
        if same {
            std::fs::metadata(f).map_or(0, |m| m.len())
        } else {
            0
        }
    };
    let theirs = {
        let mut first = vec![0u8; head.len()];
        c.open().and_then(|mut r| r.read_exact(&mut first)).is_ok() && first == head.as_bytes()
    };
    if theirs && m.len() > ours(p) {
        let part = p.with_extension("carried");
        let r = p
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .map_err(anyhow::Error::from)
            .and_then(|()| c.copy_to_local(&part));
        match r {
            Err(e) => {
                eprintln!("resume: {} not taken: {e:#}", c.display());
                std::fs::remove_file(&part).ok();
                return;
            }
            Ok(n) if ours(&part) > ours(p) => match std::fs::rename(&part, p) {
                Ok(()) => {
                    eprintln!("resume: {}: {} MB carried from the NAS, as another run left it", p.display(), n >> 20)
                }
                Err(e) => eprintln!("resume: {} not taken: {e}", part.display()),
            },
            // (Changed as it came, or no longer more than this Mac's own: this Mac's kept.)
            Ok(_) => {}
        }
        std::fs::remove_file(&part).ok();
    }
    if let Err(e) = c.remove_file() {
        eprintln!("resume: {} not deleted: {e}", c.display());
    }
    // (Its target's folder, once it holds no other log.)
    if let Some(d) = c.parent() {
        d.remove_dir().ok();
    }
}

/// Whether this job keeps its blocks: its agent turned resuming on.
pub fn on() -> bool {
    std::env::var_os(RESUME_ENV).is_some()
}

/// Target `t` (z, x, y)'s log in scratch folder `scratch`.
pub fn path(scratch: &Path, t: (u8, u32, u32)) -> PathBuf {
    scratch.join(format!("{}_{}_{}{EXT}", t.0, t.1, t.2))
}

/// A log's name's ending.
const EXT: &str = ".resume";

/// A log's first line: `<kind>-resume 1 <id>` and a newline.
fn head(kind: &str, id: &str) -> String {
    format!("{kind}-resume 1 {id}\n")
}

/// Where in a log's first line its form is (`head`'s `1`): `L` once it holds work kept beside it
/// on this Mac's disk (`local_head`).
fn form_at(head: &str) -> usize {
    head.find("-resume 1 ").map_or(0, |i| i + "-resume ".len())
}

/// `head` as a log holding work kept beside it says it (`<kind>-resume L <id>`: `Pieces::folder_piece`),
/// the same length: never carried to another Mac, nor taken from one (`carry_out`, `take_carried`).
fn local_head(head: &str) -> String {
    let mut h = head.as_bytes().to_vec();
    h[form_at(head)] = b'L';
    String::from_utf8(h).unwrap_or_default()
}

/// Whether `first` is `head`, in either form (`local_head`).
fn is_head(first: &[u8], head: &str) -> bool {
    first == head.as_bytes() || first == local_head(head).as_bytes()
}

/// A run's identity: a digest of `parts`.
pub fn id(parts: &[&str]) -> String {
    let mut h = blake3::Hasher::new();
    for p in parts {
        h.update(&(p.len() as u64).to_le_bytes());
        h.update(p.as_bytes());
    }
    h.finalize().to_hex().to_string()
}

/// A `Pieces` run's identity (task #889): `id` of its step's kind, `versions` (the step's declared
/// output version(s), as its target's key names them), its target, and `parts` (what it reads, its
/// step's choosing: the manifest's names of its reads, `names`). As #885's logs (depth, terrain):
/// what decides the output, never the program running it, so a run resumed in a newly installed
/// app (the most common pause) takes the pieces its last run kept; code that changes the output
/// bumps its step's version, which keys the target afresh and so the log too.
pub fn job_id(kind: &str, versions: &str, target: &str, parts: &[&str]) -> String {
    let mut v = vec![kind, versions, target];
    v.extend_from_slice(parts);
    id(&v)
}

/// What a manifest says a logical name is (its content's name), when it names it: the build's
/// records (pipeline's out::Out).
pub trait Manifest {
    fn get(&self, logical: &str) -> Option<&str>;
}

/// What the manifest names `logicals` (each `<logical>=<content>`, `-` for none), a line each: the
/// inputs part of a `job_id`.
pub fn names(out: &(impl Manifest + ?Sized), logicals: impl IntoIterator<Item = String>) -> String {
    logicals.into_iter().map(|l| format!("{}={}", l, out.get(&l).unwrap_or("-"))).collect::<Vec<_>>().join("\n")
}

#[cfg(any(test, feature = "testkit"))]
thread_local! {
    /// Tests: the run stops as a pause stops it (crate::control::Paused) once this many blocks more
    /// are kept.
    pub static STOP_AFTER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// The calling thread's `STOP_AFTER`, carried to the threads its work keeps blocks on (task #1097:
/// a terrain level's tiles made, and their blocks kept, on threads beside its sampling), each keep
/// counting it down as on the caller; `back` hands what's left to the caller.
pub struct StopCarry(#[cfg(any(test, feature = "testkit"))] std::sync::Mutex<Option<usize>>);

// (The count exists only where `STOP_AFTER` does, tests and the testkit; elsewhere a carry is
// empty and `on` just runs `f`.)
impl StopCarry {
    /// The calling thread's.
    pub fn here() -> StopCarry {
        #[cfg(any(test, feature = "testkit"))]
        return StopCarry(std::sync::Mutex::new(STOP_AFTER.get()));
        #[cfg(not(any(test, feature = "testkit")))]
        StopCarry()
    }

    /// `f` (a block kept) on this thread, under the caller's count.
    pub fn on<R>(&self, f: impl FnOnce() -> R) -> R {
        #[cfg(any(test, feature = "testkit"))]
        {
            let mut g = self.0.lock().unwrap_or_else(|e| e.into_inner());
            STOP_AFTER.set(*g);
            let r = f();
            *g = STOP_AFTER.take();
            r
        }
        #[cfg(not(any(test, feature = "testkit")))]
        f()
    }

    /// What's left, the caller's again.
    pub fn back(self) {
        #[cfg(any(test, feature = "testkit"))]
        STOP_AFTER.set(self.0.into_inner().unwrap_or_else(|e| e.into_inner()));
        #[cfg(not(any(test, feature = "testkit")))]
        let _ = self;
    }
}

/// A target's blocks kept so far, and its log open to append.
pub struct Log {
    path: PathBuf,
    file: Option<std::fs::File>,
    /// Where its first line's form is, and whether it says `local_head`'s.
    form_at: u64,
    local: bool,
    /// The blocks the log holds.
    pub done: BTreeSet<BlockId>,
}

impl Log {
    /// Target `t`'s log in `scratch` (another slot's moved here, or a fuller one carried on the NAS
    /// taken in its place: `take_carried`) for a run of step kind `kind` and
    /// identity `id`: each block it holds given to `take` in the order kept (with what was kept of
    /// it; each record starts where the one before ends, the first after `head`'s line), none when
    /// it has another identity or none. A record `take` refuses ends the log there, as a damaged
    /// one. None when it can't be written (nothing kept then). Logs left long by other targets
    /// swept first.
    pub fn open(
        scratch: &Path,
        t: (u8, u32, u32),
        kind: &str,
        id: &str,
        take: &mut dyn FnMut(BlockId, &[u8]) -> Result<()>,
    ) -> Option<Log> {
        let p = path(scratch, t);
        sweep(scratch, &p, HOLD_S);
        if !p.exists() {
            if let Some(other) = elsewhere(scratch, t) {
                if let Err(e) = std::fs::create_dir_all(scratch).and_then(|()| std::fs::rename(&other, &p)) {
                    eprintln!("resume: {} not moved here: {e}", other.display());
                }
            }
        }
        let head = head(kind, id);
        take_carried(carry_root().as_ref(), t, &p, &head);
        let r = (|| -> Result<Log> {
            std::fs::create_dir_all(scratch)?;
            let (done, good, local) = match std::fs::File::open(&p) {
                Ok(f) => {
                    let mut r = std::io::BufReader::with_capacity(1 << 20, f);
                    let mut first = vec![0u8; head.len()];
                    if r.read_exact(&mut first).is_ok() && is_head(&first, &head) {
                        let (done, n) = read(&mut r, take);
                        (done, (head.len() as u64) + n, first != head.as_bytes())
                    } else {
                        (BTreeSet::new(), 0, false)
                    }
                }
                Err(_) => (BTreeSet::new(), 0, false),
            };
            let mut f = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(&p)?;
            // (Whatever follows the last whole record cut; another identity's, all of it.)
            f.set_len(good)?;
            std::io::Seek::seek(&mut f, std::io::SeekFrom::End(0))?;
            if good == 0 {
                f.write_all(head.as_bytes())?;
            }
            let at_open = good.max(head.len() as u64);
            // (Weighed by `carry_out` should the job stop short: once a log, as it opened first.)
            with_opened(|v| {
                if !v.iter().any(|o| o.path == p) {
                    let (opened, total) = (std::time::Instant::now(), done.len() as u64);
                    v.push(Opened {
                        path: p.clone(),
                        t,
                        at_open,
                        opened,
                        first_keep: None,
                        last_keep: None,
                        new: 0,
                        total,
                        local,
                        #[cfg(any(test, feature = "testkit"))]
                        thread: std::thread::current().id(),
                    });
                }
            });
            Ok(Log { path: p.clone(), file: Some(f), form_at: form_at(&head) as u64, local, done })
        })();
        match r {
            Ok(l) => {
                if !l.done.is_empty() {
                    eprintln!("resume: {kind} {}/{}/{}: {} blocks kept by an earlier run", t.0, t.1, t.2, l.done.len());
                    // (The work this run skips, in its timings' record: crate::timings::Done; once a
                    // log, however often this process opens it again.)
                    static COUNTED: std::sync::Mutex<BTreeSet<PathBuf>> = std::sync::Mutex::new(BTreeSet::new());
                    if COUNTED.lock().unwrap_or_else(|e| e.into_inner()).insert(l.path.clone()) {
                        crate::timings::resumed(l.done.len() as u64);
                    }
                }
                Some(l)
            }
            Err(e) => {
                eprintln!("resume: {}: {e:#}; nothing kept", p.display());
                std::fs::remove_file(&p).ok();
                None
            }
        }
    }

    /// Block `b` finished, what its step keeps of it `payload`: appended. A failure to keep it is
    /// said and the run goes on keeping nothing more. Once it's kept, a safe point (task #885,
    /// crate::control::piece_saved): Err(crate::control::Paused) with the build pausing, the run
    /// resumed from here; a block not kept (the log failed) is no safe point, as its work would be
    /// lost.
    pub fn keep(&mut self, b: BlockId, payload: &[u8]) -> Result<()> {
        if let Some(f) = self.file.as_mut() {
            match f.write_all(&record(b, payload)) {
                Ok(()) => {
                    crate::timings::made();
                    let new = !self.done.contains(&b);
                    with_opened(|v| {
                        if let Some(o) = v.iter_mut().find(|o| o.path == self.path) {
                            let now = std::time::Instant::now();
                            o.first_keep.get_or_insert(now);
                            (o.last_keep, o.new, o.total) = (Some(now), o.new + 1, o.total + u64::from(new));
                        }
                    });
                }
                Err(e) => {
                    eprintln!("resume: {} not kept further: {e}", self.path.display());
                    self.file = None;
                }
            }
        }
        self.done.insert(b);
        #[cfg(any(test, feature = "testkit"))]
        if let Some(n) = STOP_AFTER.get() {
            if n <= 1 {
                STOP_AFTER.set(None);
                // (As a pause stops it: the same error, at the same point.)
                return Err(anyhow::Error::new(crate::control::Paused)
                    .context(format!("stopped (a test's pause) after block {b:?}")));
            }
            STOP_AFTER.set(Some(n - 1));
        }
        if self.file.is_some() {
            crate::control::piece_saved()?;
        }
        Ok(())
    }

    /// The log holds work kept beside it on this Mac's disk: its first line says so
    /// (`local_head`), once.
    fn mark_local(&mut self) {
        use std::os::unix::fs::FileExt;
        let Some(f) = self.file.as_ref().filter(|_| !self.local) else { return };
        if let Err(e) = f.write_all_at(b"L", self.form_at) {
            eprintln!("resume: {} not marked as this Mac's: {e}; kept no further", self.path.display());
            self.file = None;
            return;
        }
        self.local = true;
        with_opened(|v| v.iter_mut().filter(|o| o.path == self.path).for_each(|o| o.local = true));
    }
}

/// A target's work as pieces, each piece's result kept as it's made (task #889): the shared path of
/// the steps whose target is a run of independent pieces (a chunk of tiles sampled, a z6 tile's
/// part, a batch of a pack's tiles), each a pure function of the run's inputs (its identity) and
/// its own id. `piece` hands back a piece kept by an earlier run of the same identity, else makes
/// it and keeps it: a safe point once kept (`Log::keep`). A resumed run makes only the pieces its
/// last didn't keep, and as each is the same bytes, so is the target's output. Without a log
/// (resuming off, or the log unwritable) it just makes each.
pub struct Pieces {
    log: Option<Log>,
    /// Each piece an earlier run kept: where its record starts in the log and its payload's length,
    /// read as it's taken (`read_kept`).
    kept: std::collections::HashMap<BlockId, (u64, u64)>,
    /// The last piece the earlier run kept (`folders_as_kept`).
    last: Option<BlockId>,
}

impl Pieces {
    /// Target `t`'s pieces in `scratch` for a run of step kind `kind` and identity `id` (`id()`),
    /// kept only with `keep` (the job's agent turned resuming on, `on()`; tests say so).
    pub fn open(scratch: &Path, t: (u8, u32, u32), kind: &str, id: &str, keep: bool) -> Pieces {
        let (mut kept, mut last) = (std::collections::HashMap::new(), None);
        let mut at = head(kind, id).len() as u64;
        let log = if keep {
            Log::open(scratch, t, kind, id, &mut |b, p| {
                kept.insert(b, (at, p.len() as u64));
                last = Some(b);
                at += RECORD_TOP + p.len() as u64;
                Ok(())
            })
        } else {
            None
        };
        if log.is_none() {
            (kept, last) = (Default::default(), None);
        }
        Pieces { log, kept, last }
    }

    /// Pieces never kept (each just made): a run by hand, a test's.
    pub fn none() -> Pieces {
        Pieces { log: None, kept: Default::default(), last: None }
    }

    /// A piece whose work is files in folders `dirs` (kept on this Mac's disk between runs): `make`'s
    /// payload kept after the folders' digest once it has run (`folder_digest`; `UNREAD` for folders
    /// not read whole), handed back without it. A step made of such pieces (a z6 tile's preparing
    /// and tail, roads::unit) takes what an earlier run kept only while `folders_as_kept`.
    pub fn folder_piece(
        &mut self,
        b: BlockId,
        dirs: &[&Path],
        make: impl FnOnce() -> Result<Vec<u8>>,
    ) -> Result<Vec<u8>> {
        // (Marked before its first such piece is kept: a stop right after it never carries it.)
        if let Some(l) = self.log.as_mut() {
            l.mark_local();
        }
        let p = self.piece(b, || {
            let m = make()?;
            // (The digest once it has run: what it left.)
            Ok([folder_digest(dirs).unwrap_or(UNREAD).as_slice(), &m].concat())
        })?;
        Ok(p.get(32..).map(<[u8]>::to_vec).unwrap_or_default())
    }

    /// Whether folders `dirs` are as the last `folder_piece` an earlier run kept left them (none
    /// kept: true), and read whole: a link naming what isn't there, or an entry that can't be read,
    /// is never taken as kept (`folder_digest`). Else the step starts afresh.
    pub fn folders_as_kept(&self, dirs: &[&Path]) -> bool {
        let Some(last) = self.last.filter(|b| self.kept.contains_key(b)) else { return true };
        let Some(now) = folder_digest(dirs) else { return false };
        self.read_kept(last).is_some_and(|p| p.get(..32) == Some(now.as_slice()))
    }

    /// Pieces of floats (`piece`, the floats by their bits).
    pub fn floats(&mut self, b: BlockId, make: impl FnOnce() -> Result<Vec<Vec<f32>>>) -> Result<Vec<Vec<f32>>> {
        let p = self.piece(b, || {
            let mut p = Put::default();
            let v = make()?;
            p.u64(v.len() as u64);
            for e in &v {
                p.f32s(e);
            }
            Ok(p.0)
        })?;
        let mut g = Get(&p);
        let v = (0..g.count(8)?).map(|_| g.f32s()).collect::<Result<_>>()?;
        g.end()?;
        Ok(v)
    }

    /// Whether an earlier run kept piece `b`.
    pub fn kept(&self, b: BlockId) -> bool {
        self.kept.contains_key(&b)
    }

    /// What an earlier run kept of piece `b`, read from the log (not taken): None when it kept
    /// none, or its record no longer reads back whole.
    pub fn read_kept(&self, b: BlockId) -> Option<Vec<u8>> {
        use std::os::unix::fs::FileExt;
        let (&(at, len), log) = (self.kept.get(&b)?, self.log.as_ref()?);
        let mut r = vec![0u8; usize::try_from(RECORD_TOP + len).ok()?];
        std::fs::File::open(&log.path).and_then(|f| f.read_exact_at(&mut r, at)).ok()?;
        let p = &r[40..];
        (u64::from_le_bytes(r[..8].try_into().ok()?) == 9 + len
            && blake3::hash(p).as_bytes() == &r[8..40]
            && p[..9] == [&[b.0][..], &b.1.to_le_bytes(), &b.2.to_le_bytes()].concat()[..])
            .then(|| p[9..].to_vec())
    }

    /// Piece `b`'s result: kept by an earlier run, else `make`'s, kept (a safe point:
    /// Err(crate::control::Paused) with the build pausing, once it's kept).
    pub fn piece(&mut self, b: BlockId, make: impl FnOnce() -> Result<Vec<u8>>) -> Result<Vec<u8>> {
        if let Some(p) = self.take(b) {
            return Ok(p);
        }
        let p = make()?;
        self.keep(b, &p)?;
        Ok(p)
    }

    /// What an earlier run kept of piece `b`, taken: for pieces made side by side (`piece`'s halves,
    /// the pieces held behind a lock only to take and keep them, never while one is made).
    pub fn take(&mut self, b: BlockId) -> Option<Vec<u8>> {
        let p = self.read_kept(b);
        self.kept.remove(&b);
        p
    }

    /// Piece `b` made, `p` its result: kept (a safe point, as `piece`'s).
    pub fn keep(&mut self, b: BlockId, p: &[u8]) -> Result<()> {
        // (Not kept, no safe point: a stop here would lose the target's pieces.)
        match self.log.as_mut() {
            Some(l) => l.keep(b, p),
            None => Ok(()),
        }
    }

    /// The target's output saved: its log deleted.
    pub fn finish(self) {
        if let Some(l) = self.log {
            std::fs::remove_file(&l.path).ok();
        }
    }
}

/// The digest a `folder_piece` keeps for folders it couldn't read whole: no folders' digest, so
/// never taken as kept.
const UNREAD: [u8; 32] = [0; 32];

/// Folders `dirs`' digest: each file's path in them, length and time written (what a program
/// changes; a link's own, not its file's), in order. A folder of `dirs` not there has none. None
/// when they can't be read whole: a folder or entry that can't be listed or looked at, or a link
/// naming a file that isn't there (room-making may delete a cache file a folder links to, once
/// no job holds it: store::cachefile::hold_links).
fn folder_digest(dirs: &[&Path]) -> Option<[u8; 32]> {
    fn walk(d: &Path, rel: &str, top: bool, h: &mut blake3::Hasher) -> Option<()> {
        let mut es = match std::fs::read_dir(d) {
            Ok(es) => es.collect::<std::io::Result<Vec<_>>>().ok()?,
            Err(e) if top && e.kind() == std::io::ErrorKind::NotFound => return Some(()),
            Err(_) => return None,
        };
        es.sort_by_key(|e| e.file_name());
        for e in es {
            let name = format!("{rel}/{}", e.file_name().to_string_lossy());
            // (The entry itself, a link not followed.)
            let m = e.metadata().ok()?;
            if m.is_dir() {
                walk(&e.path(), &name, false, h)?;
                continue;
            }
            if m.file_type().is_symlink() {
                std::fs::metadata(e.path()).ok()?;
            }
            let at = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).unwrap_or_default();
            h.update(name.as_bytes());
            h.update(&m.len().to_le_bytes());
            h.update(&at.as_nanos().to_le_bytes());
        }
        Some(())
    }
    let mut h = blake3::Hasher::new();
    for (k, d) in dirs.iter().enumerate() {
        walk(d, &k.to_string(), true, &mut h)?;
    }
    Some(*h.finalize().as_bytes())
}

/// A record's bytes before what its step keeps: its payload's length and BLAKE3 (40), the block's
/// z, x and y (9).
const RECORD_TOP: u64 = 49;

/// A record: its payload's length, its BLAKE3, the payload (the block's z, x and y, then what its
/// step keeps).
fn record(b: BlockId, payload: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(9 + payload.len());
    p.push(b.0);
    p.extend(b.1.to_le_bytes());
    p.extend(b.2.to_le_bytes());
    p.extend_from_slice(payload);
    let mut r = Vec::with_capacity(40 + p.len());
    r.extend((p.len() as u64).to_le_bytes());
    r.extend(blake3::hash(&p).as_bytes());
    r.extend(p);
    r
}

/// The most a record may hold: a length past it is damage, not a record (a block's tiles are tens
/// of MB).
const MOST: u64 = 4 << 30;

/// A log's records (after its first line), read from `r` one at a time, each whole one given to
/// `take`: their blocks and the length of the whole records `take` took.
fn read(r: &mut impl Read, take: &mut dyn FnMut(BlockId, &[u8]) -> Result<()>) -> (BTreeSet<BlockId>, u64) {
    let mut done = BTreeSet::new();
    let mut at = 0u64;
    loop {
        let mut top = [0u8; 40];
        if r.read_exact(&mut top).is_err() {
            break;
        }
        let len = u64::from_le_bytes(top[..8].try_into().unwrap());
        if !(9..=MOST).contains(&len) {
            break;
        }
        let mut p = Vec::new();
        if r.take(len).read_to_end(&mut p).is_err() || p.len() as u64 != len {
            break;
        }
        if blake3::hash(&p).as_bytes() != &top[8..] {
            break;
        }
        let b =
            (p[0], u32::from_le_bytes(p[1..5].try_into().unwrap()), u32::from_le_bytes(p[5..9].try_into().unwrap()));
        if let Err(e) = take(b, &p[9..]) {
            eprintln!("resume: block {b:?} not taken: {e:#}; the log ends there");
            break;
        }
        done.insert(b);
        at += 40 + len;
    }
    (done, at)
}

/// The same step's folders of this agent's other slots (scratch folder `scratch` is
/// `<home>/scratch[-n]/<step>`), and `scratch`'s own first; just its own when it isn't one.
fn step_folders(scratch: &Path) -> Vec<PathBuf> {
    let mut v = vec![scratch.to_path_buf()];
    let (Some(step), Some(slot)) = (scratch.file_name(), scratch.parent()) else { return v };
    let Some(home) = slot.parent() else { return v };
    if !slot.file_name().is_some_and(|n| n.to_string_lossy().starts_with("scratch")) {
        return v;
    }
    let mut others: Vec<PathBuf> = std::fs::read_dir(home)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("scratch") && e.path() != slot)
        .map(|e| e.path().join(step))
        .collect();
    others.sort();
    v.extend(others);
    v
}

/// Target `t`'s log in the same step's folder of another of this agent's slots, if one has it.
fn elsewhere(scratch: &Path, t: (u8, u32, u32)) -> Option<PathBuf> {
    step_folders(scratch).into_iter().skip(1).map(|d| path(&d, t)).find(|p| p.is_file())
}

/// The logs in the step's folders (`step_folders`) but `keep` not written for `age_s`, deleted: a
/// target made since on another Mac never comes back to them (a log being written is touched
/// each block, far more often).
fn sweep(scratch: &Path, keep: &Path, age_s: u64) {
    let now = std::time::SystemTime::now();
    for d in step_folders(scratch) {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p == keep || !e.file_name().to_string_lossy().ends_with(EXT) {
                continue;
            }
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|m| now.duration_since(m).ok())
                .is_some_and(|a| a.as_secs() >= age_s);
            if old && e.file_type().is_ok_and(|t| t.is_file()) && std::fs::remove_file(&p).is_ok() {
                eprintln!("resume: {} left {age_s} s, deleted", p.display());
            }
        }
    }
}

/// A payload written field by field, little-endian (what the steps keep of a block).
#[derive(Default)]
pub struct Put(pub Vec<u8>);

impl Put {
    pub fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    pub fn u32(&mut self, v: u32) {
        self.0.extend(v.to_le_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.0.extend(v.to_le_bytes());
    }
    /// Bytes, with their length.
    pub fn bytes(&mut self, v: &[u8]) {
        self.u64(v.len() as u64);
        self.0.extend_from_slice(v);
    }
    /// Floats by their bits, with their count.
    pub fn f32s(&mut self, v: &[f32]) {
        self.u64(v.len() as u64);
        for x in v {
            self.0.extend(x.to_bits().to_le_bytes());
        }
    }
}

/// A payload read back field by field (`Put`'s); an error where it's cut short.
pub struct Get<'a>(pub &'a [u8]);

impl<'a> Get<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        anyhow::ensure!(self.0.len() >= n, "cut short");
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into()?))
    }
    /// A count of items each at least `each` bytes: an error past what's left.
    pub fn count(&mut self, each: usize) -> Result<usize> {
        let n = usize::try_from(self.u64()?)?;
        anyhow::ensure!(n.saturating_mul(each.max(1)) <= self.0.len(), "a count past the end");
        Ok(n)
    }
    pub fn bytes(&mut self) -> Result<Vec<u8>> {
        let n = self.count(1)?;
        Ok(self.take(n)?.to_vec())
    }
    pub fn f32s(&mut self) -> Result<Vec<f32>> {
        let n = self.count(4)?;
        Ok(self
            .take(n * 4)?
            .chunks_exact(4)
            .map(|c| f32::from_bits(u32::from_le_bytes(c.try_into().unwrap())))
            .collect())
    }
    /// Whether all was read.
    pub fn end(&self) -> Result<()> {
        anyhow::ensure!(self.0.is_empty(), "{} bytes past the end", self.0.len());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A log's blocks and payloads as opened.
    fn opened(scratch: &Path, t: (u8, u32, u32), id: &str) -> (Log, BTreeMap<BlockId, Vec<u8>>) {
        let mut m = BTreeMap::new();
        let l = Log::open(scratch, t, "test", id, &mut |b, p| {
            m.insert(b, p.to_vec());
            Ok(())
        })
        .unwrap();
        (l, m)
    }

    /// A log of three blocks for target `t` in `scratch`.
    fn three(scratch: &Path, t: (u8, u32, u32), id: &str) {
        let mut l = opened(scratch, t, id).0;
        l.keep((9, 152, 184), &[1, 2, 3]).unwrap();
        l.keep((9, 160, 184), &[]).unwrap();
        l.keep((10, 304, 368), &[4]).unwrap();
    }

    #[test]
    fn a_log_keeps_whole_records_and_drops_a_tail_cut_short() {
        let d = tempfile::tempdir().unwrap();
        let scratch = d.path().join("home/scratch/depth");
        let t = (6, 19, 23);
        three(&scratch, t, "a");
        let (l, m) = opened(&scratch, t, "a");
        assert_eq!(l.done.len(), 3);
        assert_eq!(m, [((9, 152, 184), vec![1, 2, 3]), ((9, 160, 184), vec![]), ((10, 304, 368), vec![4])].into());
        drop(l);
        // (The last record cut short, as a stop mid-write.)
        let p = path(&scratch, t);
        let n = std::fs::metadata(&p).unwrap().len();
        std::fs::OpenOptions::new().write(true).open(&p).unwrap().set_len(n - 3).unwrap();
        let (mut l, m) = opened(&scratch, t, "a");
        assert_eq!(l.done, [(9, 152, 184), (9, 160, 184)].into());
        assert_eq!(m.len(), 2);
        // Appended after the cut: whole again.
        l.keep((9, 168, 184), &[]).unwrap();
        drop(l);
        assert_eq!(opened(&scratch, t, "a").0.done.len(), 3);
        // Another identity, or another step's kind: afresh, the log emptied.
        let (l, m) = opened(&scratch, t, "b");
        assert!(l.done.is_empty() && m.is_empty());
        drop(l);
        assert!(opened(&scratch, t, "a").0.done.is_empty());
        three(&scratch, t, "a");
        let l = Log::open(&scratch, t, "other", "a", &mut |_, _| Ok(())).unwrap();
        assert!(l.done.is_empty());
    }

    #[test]
    fn a_damaged_log_keeps_the_records_before_the_damage() {
        let d = tempfile::tempdir().unwrap();
        let scratch = d.path().join("home/scratch/depth");
        let t = (6, 19, 23);
        three(&scratch, t, "a");
        let p = path(&scratch, t);
        let b = std::fs::read(&p).unwrap();
        let first = head("test", "a").len() + record((9, 152, 184), &[1, 2, 3]).len();
        // A byte of the second record's payload flipped: the first kept, the rest dropped.
        let mut bad = b.clone();
        bad[first + 45] ^= 1;
        std::fs::write(&p, &bad).unwrap();
        let (l, m) = opened(&scratch, t, "a");
        assert_eq!(l.done, [(9, 152, 184)].into());
        assert_eq!(m.len(), 1);
        drop(l);
        assert_eq!(std::fs::metadata(&p).unwrap().len() as usize, first, "cut after the last whole record");
        // A record its step refuses: the log ends before it.
        std::fs::write(&p, &b).unwrap();
        let l = Log::open(&scratch, t, "test", "a", &mut |b, _| {
            anyhow::ensure!(b.0 == 9, "no");
            Ok(())
        })
        .unwrap();
        assert_eq!(l.done.len(), 2);
        drop(l);
        // A length past the file's end, or past the most, a first line damaged, an empty file:
        // nothing more taken.
        let h = head("test", "a").len();
        for bad in [
            [&b[..first], &u64::MAX.to_le_bytes()[..], &[0u8; 64][..]].concat(),
            [&b[..first], &100_000u64.to_le_bytes()[..], &[0u8; 64][..]].concat(),
            [b"test-resume 1 x\n".as_slice(), &b[h..]].concat(),
            Vec::new(),
        ] {
            std::fs::write(&p, &bad).unwrap();
            let (l, m) = opened(&scratch, t, "a");
            assert!(l.done.len() <= 1 && m.len() == l.done.len());
        }
    }

    #[test]
    fn a_log_moves_from_another_slot_and_logs_left_long_are_swept() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("home");
        let (mine, other) = (home.join("scratch/depth"), home.join("scratch-2/depth"));
        // Another slot's log of the target, moved here (this slot's step folder not yet made).
        three(&other, (6, 1, 1), "c");
        assert_eq!(opened(&mine, (6, 1, 1), "c").0.done.len(), 3);
        assert!(!path(&other, (6, 1, 1)).exists() && path(&mine, (6, 1, 1)).exists());
        // Two slots at once, each its own target: neither touches the other's.
        let mut a = opened(&mine, (6, 2, 2), "c").0;
        let mut b = opened(&other, (6, 3, 3), "c").0;
        a.keep((9, 16, 16), &[]).unwrap();
        b.keep((9, 24, 24), &[]).unwrap();
        drop((a, b));
        assert_eq!(opened(&other, (6, 3, 3), "c").0.done.len(), 1);
        assert!(path(&mine, (6, 2, 2)).exists());
        // Swept: those older than the age, in every slot's folder, but the one opened.
        std::fs::write(other.join("notes.txt"), b"x").unwrap();
        sweep(&mine, &path(&mine, (6, 1, 1)), 0);
        assert!(path(&mine, (6, 1, 1)).exists());
        assert!(!path(&mine, (6, 2, 2)).exists() && !path(&other, (6, 3, 3)).exists());
        assert!(other.join("notes.txt").exists(), "only logs");
        sweep(&mine, &path(&mine, (6, 9, 9)), 3600);
        assert!(path(&mine, (6, 1, 1)).exists(), "a fresh one kept");
    }

    /// Task #1414: a target given back on one Mac resumes on another. The logs its job grew are
    /// carried to the NAS as it stops (`carry_out`: not one it never opened, nor one it opened and
    /// kept nothing more in), and the next run, wherever it goes, takes the copy when it holds more
    /// of the run's identity than the log there; a poorer copy or another identity's leaves the log
    /// as it is. Either way the copy is gone once looked at.
    #[test]
    fn a_log_given_back_on_one_mac_resumes_on_another() {
        let d = tempfile::tempdir().unwrap();
        let nas = store::nasfs::NasRoot::local_for_test(d.path().join("nas"));
        let [a, b, c] = ["a/scratch/depth", "b/scratch-2/depth", "c/scratch/depth"].map(|s| d.path().join(s));
        let t = (6, 19, 23);
        let copy = d.path().join("nas").join(CARRIED).join("depth-step/6%2F19%2F23/6_19_23.resume");
        // (Filed under its target as the plan names it, among the job's.)
        CARRY_ROOT.set(Some(Carry::test(&nas, "depth-step", &["6/1/1", "6/19/23"])));
        // (A new job's process on each "Mac": what this thread opened before forgotten.)
        let job = || forget_opened();
        job();
        three(&a, t, "a");
        // (A log of another target, never opened by this job: not its, not carried.)
        std::fs::write(path(&a, (6, 1, 1)), b"x").unwrap();
        let out = carry_out();
        assert_eq!(out.iter().map(|o| o.0.as_str()).collect::<Vec<_>>(), ["6_19_23.resume"]);
        assert_eq!(std::fs::read(&copy).unwrap(), std::fs::read(path(&a, t)).unwrap());
        assert!(!a.parent().unwrap().parent().unwrap().join("carry.lock").exists(), "its Mac's turn let go");
        // The other Mac's run takes it: the three blocks, as kept.
        job();
        let (mut l, m) = opened(&b, t, "a");
        assert_eq!(m, [((9, 152, 184), vec![1, 2, 3]), ((9, 160, 184), vec![]), ((10, 304, 368), vec![4])].into());
        assert!(!copy.exists(), "taken");
        // Given back there with one block more: the first Mac, its own log of three, takes the four.
        l.keep((9, 168, 184), &[5]).unwrap();
        drop(l);
        carry_out();
        job();
        let l = opened(&a, t, "a").0;
        assert_eq!(l.done.len(), 4);
        assert!(!copy.exists());
        drop(l);
        // A job that opened the log and kept nothing more in it carries nothing.
        assert!(carry_out().is_empty() && !copy.exists(), "grew no further");
        // A poorer copy (three blocks), or another identity's: the four kept here, the copy gone.
        for id in ["a", "z"] {
            std::fs::remove_dir_all(&c).ok();
            job();
            three(&c, t, id);
            carry_out();
            assert!(copy.exists());
            job();
            assert_eq!(opened(&a, t, "a").0.done.len(), 4, "{id}");
            assert!(!copy.exists(), "{id}");
        }
        // Another identity's log here, a copy of this run's: the copy taken.
        job();
        assert_eq!(opened(&a, t, "y").0.done.len(), 0);
        std::fs::remove_dir_all(&c).unwrap();
        job();
        three(&c, t, "a");
        carry_out();
        job();
        assert_eq!(opened(&a, t, "a").0.done.len(), 3, "the copy, of this identity, over another's");
        // A cancel drops its target's copy.
        job();
        three(&c, t, "a");
        carry_out();
        assert!(drop_carried(&nas, "depth-step", "6/19/23") == 1 && !copy.exists() && !copy.parent().unwrap().exists());
        CARRY_ROOT.set(None);
    }

    /// Task #1414: a log holding work kept beside it on this Mac's disk (unit's folders:
    /// `Pieces::folder_piece`) says so in its first line and is never carried; its own Mac's next
    /// run takes its pieces as before.
    #[test]
    fn a_log_of_work_kept_beside_it_stays_on_its_mac() {
        let d = tempfile::tempdir().unwrap();
        let nas = store::nasfs::NasRoot::local_for_test(d.path().join("nas"));
        let (s, dir) = (d.path().join("scratch/unit"), d.path().join("work"));
        std::fs::create_dir_all(&dir).unwrap();
        CARRY_ROOT.set(Some(Carry::test(&nas, "unit", &["6/1/2"])));
        forget_opened();
        let t = (6, 1, 2);
        let mut p = Pieces::open(&s, t, "unit", "a", true);
        p.piece((1, 0, 0), || Ok(vec![1])).unwrap();
        p.folder_piece((2, 0, 0), &[&dir], || Ok(vec![2])).unwrap();
        drop(p);
        let head = std::fs::read(path(&s, t)).unwrap();
        assert!(head.starts_with(b"unit-resume L a\n"), "{:?}", String::from_utf8_lossy(&head[..16]));
        assert!(carry_out().is_empty());
        assert!(!d.path().join("nas").join(CARRIED).exists());
        forget_opened();
        let p = Pieces::open(&s, t, "unit", "a", true);
        assert!(p.kept((1, 0, 0)) && p.kept((2, 0, 0)) && p.folders_as_kept(&[&dir]));
        CARRY_ROOT.set(None);
    }

    /// Task #1414: a log is carried only when its copy, up and down at the job's NAS rate, takes
    /// less than the work it holds; with no rate measured, carried.
    #[test]
    fn a_log_is_carried_only_when_its_copy_costs_less_than_its_work() {
        let mb = 1_000_000;
        assert!(worth_carrying(100 * mb, Some(10.0 * mb as f64), 20.0));
        assert!(!worth_carrying(100 * mb, Some(10.0 * mb as f64), 19.0));
        assert!(worth_carrying(100 * mb, None, 0.0));
    }

    /// Task #1414: a log is filed under the job's target its tile names, however the plan names it
    /// (`n/4/3/1` for `4_3_1`), else under the job's only target; a log no target names, not at all.
    #[test]
    fn a_log_is_filed_under_the_target_the_plan_names() {
        let nas = store::nasfs::NasRoot::local_for_test("/tmp/x-nas-test");
        let c = Carry::test(&nas, "polar-terrain", &["n/4/3/1", "s/4/3/2"]);
        assert_eq!(c.target_of((4, 3, 1)), Some("n/4/3/1"));
        assert_eq!(c.target_of((4, 3, 2)), Some("s/4/3/2"));
        assert_eq!(c.target_of((4, 9, 9)), None);
        assert_eq!(Carry::test(&nas, "water", &["2026-11-01"]).target_of((0, 0, 0)), Some("2026-11-01"));
        assert_eq!(target_of_folder(&folder_of("n/4/3%/1")), "n/4/3%/1");
        assert!(!folder_of("n/4/3/1").contains('/'));
    }

    /// Task #1414: a Mac's carry turn is waited for only so long: past it, nothing.
    #[test]
    fn a_carry_waits_for_its_macs_turn_only_so_long() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("carry.lock");
        let held = crate::sys::PathLock::take(&lock, false).unwrap().unwrap();
        let t = std::time::Instant::now();
        assert!(turn_within(&lock, std::time::Duration::from_millis(300)).is_none());
        assert!(t.elapsed() >= std::time::Duration::from_millis(300));
        drop(held);
        assert!(turn_within(&lock, std::time::Duration::ZERO).is_some());
    }

    /// Task #1414: the lead's sweep of the carried copies deletes a copy whose target its plan
    /// neither plans nor leases, and a copy cut short (its temporary file) once it's a day old.
    #[test]
    fn copies_no_run_will_take_and_copies_cut_short_are_swept() {
        let d = tempfile::tempdir().unwrap();
        let nas = store::nasfs::NasRoot::local_for_test(d.path().join("nas"));
        let dir = d.path().join("nas").join(CARRIED).join("bldtiles");
        let day = store::whole::ASIDE_KEPT;
        let at = |name: &str, age: std::time::Duration| {
            let p = dir.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"x").unwrap();
            let f = std::fs::File::options().write(true).open(&p).unwrap();
            f.set_modified(std::time::SystemTime::now() - age).unwrap();
            p
        };
        let zero = std::time::Duration::ZERO;
        let planned = at("6%2F1%2F2/6_1_2.resume", zero);
        let gone = at("6%2F3%2F4/6_3_4.resume", zero);
        let cut = at("6%2F1%2F2/6_5_6.resume.mac.4242.0.tmp", day);
        let young = at("6%2F1%2F2/6_7_8.resume.mac.4243.0.tmp", zero);
        let n = sweep_carried(&nas, &|step, t| step == "bldtiles" && t == "6/1/2");
        assert_eq!(n, 2);
        assert!(
            planned.exists() && !gone.exists() && !gone.parent().unwrap().exists() && !cut.exists() && young.exists()
        );
    }

    /// Task #1414: a job stopping at a safe point has carried its log before it exits, whichever
    /// way it comes to its paused exit (crate::control::stop_paused, or crate::control::exit's
    /// PAUSED_EXIT, as the trees program's own): its agent sees its end, and gives its lease back,
    /// only after; and it said it was carrying (crate::control::carrying_mark), so its agent
    /// doesn't freeze it meanwhile. Drained for memory (crate::control::no_carry_mark), nothing.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_job_stopping_at_a_safe_point_carries_its_log_before_it_exits() {
        const CHILD: &str = "SCENIC_TEST_CARRY_SCRATCH";
        const BY_EXIT: &str = "SCENIC_TEST_CARRY_BY_EXIT";
        if let Some(s) = std::env::var_os(CHILD) {
            let mut p = Pieces::open(Path::new(&s), (6, 1, 2), "test", "a", true);
            p.piece((1, 0, 0), || Ok(vec![1, 2])).unwrap();
            if std::env::var_os(BY_EXIT).is_some() {
                crate::control::exit(crate::control::PAUSED_EXIT);
            }
            crate::control::stop_paused("test");
        }
        let d = tempfile::tempdir().unwrap();
        let chan = d.path().join("control");
        let run = |name: &str, by_exit: bool| {
            let s = d.path().join(format!("home/scratch/{name}"));
            let mut c = store::spawn::command(std::env::current_exe().unwrap());
            c.args(["--exact", "resume_log::tests::a_job_stopping_at_a_safe_point_carries_its_log_before_it_exits"])
                .env(CHILD, &s)
                .env(CARRY_ENV, d.path().join("nas"))
                .env(CARRY_STEP_ENV, name)
                .env(CARRY_TARGETS_ENV, "6/1/2")
                .env(crate::control::CONTROL_ENV, &chan)
                .env_remove(crate::timings::TIMINGS_ENV);
            if by_exit {
                c.env(BY_EXIT, "1");
            }
            let st = store::spawn::spawn(&mut c).unwrap().wait().unwrap();
            assert_eq!(st.code(), Some(crate::control::PAUSED_EXIT), "{st:?}");
            (s, d.path().join("nas").join(CARRIED).join(format!("{name}/6%2F1%2F2/6_1_2.resume")))
        };
        for by_exit in [false, true] {
            let name = if by_exit { "by-exit" } else { "stopped" };
            std::fs::remove_file(crate::control::carrying_mark(&chan)).ok();
            let (s, copy) = run(name, by_exit);
            assert_eq!(std::fs::read(copy).unwrap(), std::fs::read(path(&s, (6, 1, 2))).unwrap(), "{name}");
            assert!(crate::control::carrying_mark(&chan).exists(), "{name}");
        }
        std::fs::write(crate::control::no_carry_mark(&chan), b"").unwrap();
        let (s, copy) = run("memory", false);
        assert!(path(&s, (6, 1, 2)).exists() && !copy.exists(), "drained for memory: nothing carried");
    }

    /// Task #1562: a z6 tile's folders kept by its last run, one of them links to cache files, are
    /// taken up only while every link names a file that's there and every entry reads.
    #[test]
    fn a_kept_folder_with_a_link_to_what_is_gone_or_an_entry_unread_is_made_again() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let (s, dir, bdir, cache) =
            (d.path().join("scratch"), d.path().join("u"), d.path().join("u-buildings"), d.path().join("cache"));
        for f in [&dir, &bdir, &cache] {
            std::fs::create_dir_all(f).unwrap();
        }
        std::fs::write(dir.join("ways.bin"), b"w").unwrap();
        std::fs::write(cache.join("8-1-2.f32"), b"b").unwrap();
        std::os::unix::fs::symlink(cache.join("8-1-2.f32"), bdir.join("8-1-2.f32")).unwrap();
        let t = (6, 1, 2);
        let dirs = [dir.as_path(), bdir.as_path()];
        let mut p = Pieces::open(&s, t, "unit", "a", true);
        p.folder_piece((6, 0, 0), &dirs, || Ok(vec![1])).unwrap();
        drop(p);
        let open = || Pieces::open(&s, t, "unit", "a", true);
        assert!(open().folders_as_kept(&dirs));
        // The file a link names deleted (room-making, once no job held it): made again.
        std::fs::remove_file(cache.join("8-1-2.f32")).unwrap();
        assert!(!open().folders_as_kept(&dirs));
        // There again (filled again: its link is as it was), taken.
        std::fs::write(cache.join("8-1-2.f32"), b"b").unwrap();
        assert!(open().folders_as_kept(&dirs));
        // A folder in them that can't be read: made again, never passed over.
        std::fs::create_dir(dir.join("scache")).unwrap();
        let mut p = open();
        p.folder_piece((6, 1, 0), &dirs, || Ok(vec![2])).unwrap();
        drop(p);
        assert!(open().folders_as_kept(&dirs));
        std::fs::set_permissions(dir.join("scache"), std::fs::Permissions::from_mode(0o000)).unwrap();
        assert!(!open().folders_as_kept(&dirs));
        // Kept while unread: never taken, even once it can be read.
        let mut p = open();
        p.folder_piece((6, 1, 1), &dirs, || Ok(vec![3])).unwrap();
        drop(p);
        std::fs::set_permissions(dir.join("scache"), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!open().folders_as_kept(&dirs));
    }

    /// Task #889: pieces kept are handed back, not made again, to a run of the same identity; to
    /// another, made afresh; once finished, the log is gone; without keeping, each is made.
    #[test]
    fn pieces_kept_are_taken_not_made_again() {
        let d = tempfile::tempdir().unwrap();
        let t = (6, 1, 2);
        let made = std::cell::Cell::new(0);
        let make = |v: u8| {
            made.set(made.get() + 1);
            Ok(vec![v; 3])
        };
        // (The run's timings count the blocks it made and those it took from an earlier run, task
        // #1419: other tests' runs may add to them meanwhile, never take away.)
        use std::sync::atomic::Ordering::Relaxed;
        let counted = || (crate::timings::RESUMED.load(Relaxed), crate::timings::MADE.load(Relaxed));
        let at = counted();
        let mut p = Pieces::open(d.path(), t, "k", "a", true);
        assert_eq!(p.piece((9, 0, 0), || make(1)).unwrap(), [1; 3]);
        assert_eq!(p.floats((9, 0, 1), || Ok(vec![vec![1.5, -0.0], vec![]])).unwrap(), [vec![1.5, -0.0], vec![]]);
        drop(p);
        let mut p = Pieces::open(d.path(), t, "k", "a", true);
        assert_eq!(p.piece((9, 0, 0), || make(2)).unwrap(), [1; 3], "kept");
        assert_eq!(p.floats((9, 0, 1), || anyhow::bail!("made again")).unwrap(), [vec![1.5, -0.0], vec![]]);
        assert_eq!(p.piece((9, 1, 0), || make(3)).unwrap(), [3; 3]);
        assert_eq!(made.get(), 2);
        let now = counted();
        assert!(now.0 >= at.0 + 2 && now.1 >= at.1 + 3, "{at:?} then {now:?}");
        p.finish();
        assert!(!path(d.path(), t).exists(), "gone once finished");
        let mut p = Pieces::open(d.path(), t, "k", "b", true);
        assert_eq!(p.piece((9, 0, 0), || make(4)).unwrap(), [4; 3], "another identity: afresh");
        drop(p);
        let mut p = Pieces::none();
        assert_eq!(p.piece((9, 0, 0), || make(5)).unwrap(), [5; 3]);
    }

    /// Task #1182: the pieces an earlier run kept are read from its log as each is taken, not held
    /// from the start: each reads back as kept, and one whose record was damaged since is made
    /// again.
    #[test]
    fn pieces_kept_are_read_as_they_are_taken() {
        let d = tempfile::tempdir().unwrap();
        let t = (3, 4, 2);
        let mut p = Pieces::open(d.path(), t, "k", "a", true);
        for (k, n) in [(0u32, 5usize), (1, 0), (2, 70_000)] {
            p.piece((8, k, 0), || Ok(vec![k as u8 + 1; n])).unwrap();
        }
        drop(p);
        let mut p = Pieces::open(d.path(), t, "k", "a", true);
        assert!(p.kept((8, 2, 0)) && !p.kept((8, 3, 0)));
        assert_eq!(p.piece((8, 2, 0), || anyhow::bail!("made again")).unwrap(), vec![3; 70_000]);
        assert_eq!(p.piece((8, 1, 0), || anyhow::bail!("made again")).unwrap(), Vec::<u8>::new());
        // (The first piece's payload damaged under the open log: made again, the rest unharmed.)
        let path = path(d.path(), t);
        let mut b = std::fs::read(&path).unwrap();
        b[head("k", "a").len() + 49] ^= 1;
        std::fs::write(&path, &b).unwrap();
        assert_eq!(p.piece((8, 0, 0), || Ok(vec![9; 5])).unwrap(), [9; 5]);
        assert!(p.take((8, 2, 0)).is_none(), "taken once");
    }

    /// Task #889: a run's identity is what decides its output (its step, version, target and
    /// reads), never the program: pieces kept by one app are taken by the next installed (a pause
    /// to restart into an update, the most common), and a version bump starts afresh.
    #[test]
    fn pieces_outlive_a_new_program_but_not_a_new_version() {
        let d = tempfile::tempdir().unwrap();
        let t = (6, 1, 2);
        let reads = "work/x=abc\nwork/y=-";
        // (Nothing but these: no digest of the program running it, whatever its build.)
        assert_eq!(job_id("k", "3", "6/1/2", &[reads]), id(&["k", "3", "6/1/2", reads]));
        let mut p = Pieces::open(d.path(), t, "k", &job_id("k", "3", "6/1/2", &[reads]), true);
        p.piece((9, 0, 0), || Ok(vec![1; 3])).unwrap();
        drop(p);
        // (The app updated: the same step, version, target and reads.)
        let mut p = Pieces::open(d.path(), t, "k", &job_id("k", "3", "6/1/2", &[reads]), true);
        assert_eq!(p.piece((9, 0, 0), || anyhow::bail!("made again")).unwrap(), [1; 3], "kept across the update");
        drop(p);
        for other in [job_id("k", "4", "6/1/2", &[reads]), job_id("k", "3", "6/1/2", &["work/x=abd"])] {
            let mut p = Pieces::open(d.path(), t, "k", &other, true);
            assert_eq!(p.piece((9, 0, 0), || Ok(vec![2; 3])).unwrap(), [2; 3], "a new version or read: afresh");
            drop(p);
        }
    }

    #[test]
    fn a_payload_reads_back_and_a_cut_one_is_an_error() {
        let mut p = Put::default();
        p.u8(7);
        p.u32(9);
        p.bytes(&[1, 2]);
        p.f32s(&[1.5, f32::NAN, -0.0]);
        let mut g = Get(&p.0);
        assert_eq!((g.u8().unwrap(), g.u32().unwrap(), g.bytes().unwrap()), (7, 9, vec![1, 2]));
        let f = g.f32s().unwrap();
        assert_eq!(f.iter().map(|x| x.to_bits()).collect::<Vec<_>>(), [1.5f32, f32::NAN, -0.0].map(f32::to_bits));
        g.end().unwrap();
        let mut g = Get(&p.0[..p.0.len() - 1]);
        assert!(g.u8().is_ok() && g.u32().is_ok() && g.bytes().is_ok() && g.f32s().is_err());
        assert!(Get(&u64::MAX.to_le_bytes()).bytes().is_err());
    }
}
