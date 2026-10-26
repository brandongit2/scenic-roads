//! Removing replaced files from the NAS (docs/storage.md, GC): every content-named file that
//! neither the newest catalogue, nor a catalogue of the last `keep_days`, nor the build's manifest
//! nor (with the pool) its newest records or a journal entry they don't reflect yet (`unmerged`)
//! references, and that is itself older than `keep_days`; abandoned `.tmp` files; old catalogues.
//! So it runs beside other jobs: a job's files are young, or named by its entry once it's written.
//!
//! Deletions go through SMB: on this share they're permanent (no Recycle Bin entry; tested
//! 2026-10-02), and the M4 can't use SSH unattended (1Password asks each session).
//!
//! Only the folders catalogues index are swept (`base/`, `global/`, `layers/`, `hidata/`, …: the
//! first path component of every referenced file), and the sources of passes older than the newest
//! complete one (`sources/osm/<date>/`, `sources/items/<date>/`): their content-named files by the
//! same rule, the rest (the planet download) once the newer pass has been complete for `keep_days`.
//! The gate's checked copies (`sources/inputs/`, docs/inputs.md §4.9) by the same rule, kept while
//! the manifest (or the newest records) names them, an accepted index it names lists them, a
//! journal entry not yet merged does, or an index made or replaced within `keep_days` lists them
//! (`inputs_kept`; none swept when one of those can't be read).
//! What removed steps made (`RETIRED`) and the depth packs of tiles no current depth target names
//! (`DepthKeep`): the run first drops their names from the manifest through the records
//! (`drop_names`), then their files go by the same rule as any content-named file.
//! The newest pass, a planet waiting for its pass, the rest of `sources/`, the user's folders (the
//! inputs folders), state and the app are never touched. With no readable catalogue nothing is deleted.

use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};
use store::nasfs::NasPath;

#[derive(Debug, Default, Serialize)]
pub struct Report {
    // The JSON keys keep their old spelling (task #547: scripts read `scenic gc`).
    #[serde(rename = "catalogs_kept")]
    pub catalogues_kept: usize,
    #[serde(rename = "catalogs_removed")]
    pub catalogues_removed: usize,
    pub referenced: usize,
    pub files_seen: usize,
    pub removed: usize,
    pub removed_bytes: u64,
    pub tmp_removed: usize,
    /// Unreferenced but too young to remove.
    pub young: usize,
    /// The names removed steps made (`RETIRED`) this run dropped from the manifest.
    pub retired_dropped: usize,
    /// The depth packs no current depth target names (`DepthKeep`) this run dropped from the
    /// manifest (a dry run: would drop).
    pub depth_dropped: usize,
    /// Of the removed, files of retired passes' sources and of removed steps (and their bytes).
    pub retired_removed: usize,
    pub retired_bytes: u64,
    /// Of the removed, the gate's checked copies; and why none were swept, when they weren't.
    pub inputs_removed: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inputs_skipped: Option<String>,
    /// The pool's journal entries its records don't reflect yet, whose files were kept.
    pub unmerged: usize,
    /// Tombstones found and put back (or, their name taken again, removed).
    pub tombs: usize,
    /// Days of the history packed into `state/coord/history-archive/<day>.tar` this run.
    pub history_days_archived: usize,
    /// The pool's journal entries removed this run (crate::pool::journal_gc), or marked to remove.
    pub journal_removed: usize,
    pub journal_marked: usize,
    /// Days of the forecasts' archive packed into `state/coord/forecasts-archive/<YYYY-MM>.tar`.
    pub forecast_days_archived: usize,
    /// The files put back this run: left alone by its sweeps (the next run decides).
    #[serde(skip)]
    pub restored: BTreeSet<NasPath>,
    /// What it passed over and why (the agent's status says them as the job's note).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    pub dry_run: bool,
}

/// Folders never swept, whatever a catalogue says (`sources/` but its retired passes and the gate's
/// checked copies, `sources/inputs/`, swept on their own rules).
const NEVER: [&str; 7] = ["translations", "descriptions", "inputs", "state", "app", "nas", "sources"];

/// The logical-name prefixes of what removed steps made (removed means deleted): GC's run drops
/// every name the manifest has under one, in one save of the records (`drop_names`: a hand-off
/// of the `gc` job's, merged by the lead as any job's, its write-set these removals alone,
/// crate::agent::steps), and its files then go as any no catalogue, manifest or records name, once
/// old: a catalogue of the last `keep_days` naming one keeps it. Under a folder GC sweeps anyway
/// (not `NEVER`) the sweep of the folders takes them; under `sources/` only their prefix's folder
/// is looked in (`sweep_retired_names`). A step whose code is deleted adds its outputs' prefix
/// here; no step of the table may make a name under one. A prefix ends in `/` for a folder.
pub const RETIRED: [&str; 2] = [
    // (The worldwide z8 terrain and its maxima, `-v<n>` and `-v<n>-max`: its code deleted by task #670.)
    "sources/terrain-z8-",
    // (The old pipeline's data, converted once and read by nothing since; task #52.)
    "global/legacy/",
];

/// Whether logical name `l` is one a removed step made (`RETIRED`).
pub fn retired(l: &str) -> bool {
    RETIRED.iter().any(|p| l.starts_with(p))
}

/// The depth's current targets as the agent's last plan found them (`build::depth_kept`: the z3 and
/// z6 tiles the coverage and the declared sources make targets, and those leased now), written to
/// the agent's home as the `gc` job is planned (`DEPTH_KEEP_FILE`) and passed by `--depth-keep`. A
/// depth pack is dropped only when no tile of these names it (`orphan`): an assembly's and the
/// floor's shading's lo packs by their z3 tile, a z6 tile's hi packs by it. None read (no file, one
/// not fresh, one unreadable), no depth name is dropped.
#[derive(Debug, Default, Clone, PartialEq, Serialize, serde::Deserialize)]
pub struct DepthKeep {
    /// When the plan found them (unix seconds).
    pub at: u64,
    /// The z3 tiles: the assemblies' and the floor's shading's targets.
    pub lo: BTreeSet<(u32, u32)>,
    /// The z6 tiles: the `depth` step's targets.
    pub hi: BTreeSet<(u32, u32)>,
}

/// The depth's steps, whose leased targets `DepthKeep` keeps (`with_leased`).
pub const DEPTH_STEPS: [&str; 3] = ["depth-lo", "depth", "depth-shade"];

/// The file in the agent's home `DepthKeep` is written to.
pub const DEPTH_KEEP_FILE: &str = "gc-depth-keep.json";
/// How old a `DepthKeep` may be when GC reads it: an hour. The agent writes it at each plan while
/// GC is due, so a job started from that plan reads one minutes old; one older (a job restarted
/// after a sleep, a plan that stopped) drops no depth name that run.
pub const DEPTH_KEEP_FRESH_S: u64 = 3600;

impl DepthKeep {
    /// The one at `p` if fresh at `now`; why not otherwise (none there: Ok(None)).
    pub fn read(p: &std::path::Path, now: u64) -> Result<Option<DepthKeep>> {
        let b = match std::fs::read(p) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("read {}", p.display())),
        };
        let k: DepthKeep = serde_json::from_slice(&b).with_context(|| format!("parse {}", p.display()))?;
        anyhow::ensure!(
            now.saturating_sub(k.at) <= DEPTH_KEEP_FRESH_S && k.at <= now + 60,
            "the depth's targets in {} are from {} s ago, not within {DEPTH_KEEP_FRESH_S} s",
            p.display(),
            now as i64 - k.at as i64
        );
        Ok(Some(k))
    }

    /// With the targets of the depth's steps leased now (`leased`: step, target): a job running
    /// for a tile the coverage left since it was planned keeps its tile's packs this run.
    pub fn with_leased(mut self, leased: &[(String, String)]) -> DepthKeep {
        for (step, t) in leased {
            if let Some(u) = crate::legacy::TileId::parse(t) {
                match (step.as_str(), u.z) {
                    ("depth", 6) => self.hi.insert((u.x, u.y)),
                    ("depth-lo" | "depth-shade", 3) => self.lo.insert((u.x, u.y)),
                    _ => false,
                };
            }
        }
        self
    }

    /// Whether logical name `l` is a depth pack (crate::depth's layers, a z6 tile's hi or a z3
    /// tile's lo) of a tile no current target names.
    pub fn orphan(&self, l: &str) -> bool {
        use crate::depth::{CLASS_LAYER, LAYER, SHADE_LAYER};
        let Some(r) = l.strip_prefix("layers/") else { return false };
        let Some((ly, r)) = r.split_once('/') else { return false };
        if ![LAYER, CLASS_LAYER, SHADE_LAYER].contains(&ly) {
            return false;
        }
        let tile = |r: &str, z: &str| -> Option<(u32, u32)> {
            let (x, y) = r.strip_prefix(z)?.strip_prefix('-')?.split_once('-')?;
            Some((x.parse().ok()?, y.parse().ok()?))
        };
        if let Some(t) = r.strip_prefix("hi/").and_then(|r| tile(r, "6")) {
            return !self.hi.contains(&t);
        }
        if let Some(t) = r.strip_prefix("lo/").and_then(|r| tile(r, "3")) {
            return !self.lo.contains(&t);
        }
        false
    }
}

/// Whether the clean-up drops logical name `l`: a removed step's (`RETIRED`), or a depth pack no
/// current target names (`depth`).
fn dropped(l: &str, depth: Option<&DepthKeep>) -> bool {
    retired(l) || depth.is_some_and(|k| k.orphan(l))
}

/// Drops from the build's manifest at `root` the names removed steps made (`RETIRED`) and the depth
/// packs no current target names (`depth`), as the `gc` step's job (`Out`, its scratch `scratch`):
/// in the pool a hand-off the lead merges, else saved under the build lock. How many of each
/// (removed steps', depth's); none named, nothing written, so a run after is a no-op. A dry run
/// counts them from the manifest, writing nothing.
pub fn drop_names(
    root: &store::nasfs::NasRoot,
    scratch: &std::path::Path,
    depth: Option<&DepthKeep>,
    dry_run: bool,
) -> Result<(usize, usize)> {
    let count = |names: &[&String]| {
        let r = names.iter().filter(|l| retired(l)).count();
        (r, names.len() - r)
    };
    if dry_run {
        let m = crate::out::read_manifest(root)?;
        let names: Vec<&String> = m.keys().filter(|l| dropped(l, depth)).collect();
        return Ok(count(&names));
    }
    let mut out = crate::out::Out::open(root, scratch, STEP)?;
    let names: Vec<String> = out.manifest.keys().filter(|l| dropped(l, depth)).cloned().collect();
    for l in &names {
        out.remove(l);
    }
    // (Their uploads never verified leave the pending ones too: the records' pending keep a file
    // from the sweeps, and a verify would find it gone.)
    let pending: std::collections::BTreeMap<String, String> =
        crate::out::read_record(&root.join("state/build/pending.json")).context("the pending uploads")?;
    let unpend: Vec<&String> = pending.keys().filter(|c| retired(c)).collect();
    for c in &unpend {
        out.unpend(c);
    }
    if !names.is_empty() || !unpend.is_empty() {
        out.save()?;
    }
    Ok(count(&names.iter().collect::<Vec<_>>()))
}

/// What the pool's records and journal name beyond today's manifest (docs/pool.md §7.3, §7.7): the
/// newest records' manifest and pending uploads (today's files may lag them a save), and every
/// journal entry those records don't reflect yet, refused ones aside: a hand-off not merged yet
/// (its member away for weeks, its entry written but untold) keeps the files it names, its changes
/// and its uploads, so GC may run beside other jobs. Empty with the pool never on (no journal); an
/// error when any of them can't be read now, or an entry the records don't name doesn't read whole
/// (short, damaged, or not there as listed: a read cut short says nothing of the entry): GC fails
/// then, removing nothing (tried again as a failed job is, sooner than its daily run), until the
/// lead has refused the entry (one unreadable for `driver::UNREADABLE_S` awake), which the records
/// then name. With no term led (the pool switched off, or its term not readable whole), no one
/// refuses: an entry unreadable whose day is `PASS_OVER_S` past is passed over, said as a warning,
/// so the hold is bounded either way.
#[derive(Debug, Default)]
pub struct Unmerged {
    /// Content names, of the records and of the entries not merged.
    pub files: BTreeSet<String>,
    /// The (logical, content) names of those entries' changes, for the sweeps that read what a
    /// file names (the gate's indexes).
    pub changes: Vec<(String, String)>,
    /// The newest records' manifest (logical, content), which today's file may lag a save.
    pub records: Vec<(String, String)>,
    /// How many entries weren't merged.
    pub entries: usize,
    /// The entries passed over, unreadable with no term led (`PASS_OVER_S`), said.
    pub warnings: Vec<String>,
}

/// How long an entry stays unreadable, no term led, before GC passes over it: three days. An entry
/// is written whole in seconds (by a temporary name), and a lead refuses one its reads find not
/// whole for `driver::UNREADABLE_S` (an hour) awake; three days is that hour with a margin for the
/// pool switched off as a lead's Mac slept, a term handed over, or a share answering stale reads a
/// while. Passing over isn't removing: the entry's files go only when old by GC's own keep window,
/// as any file no catalogue, manifest or records name.
pub const PASS_OVER_S: u64 = 3 * 86400;
const _: () = assert!(PASS_OVER_S >= crate::pool::driver::UNREADABLE_S + 2 * 86400);

/// `now`: unix seconds, which `PASS_OVER_S` is counted to (by the entry's day, its key's: one
/// whose day is before `now - PASS_OVER_S`'s was written longer ago than that).
pub fn unmerged(root: &store::nasfs::NasRoot, now: u64) -> Result<Unmerged> {
    use crate::pool::{journal, records::Records, term};
    let mut u = Unmerged::default();
    match root.join(journal::DIR).metadata() {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(u),
        Err(e) => return Err(e).context("the journal"),
    }
    let nas = root.clone();
    let cur = term::current(&nas).context("the current term")?;
    let r = Records::newest(&nas, cur.term, true).context("the newest records")?.unwrap_or_default();
    // (Led: a term readable whole; only then does anyone refuse.)
    let led = cur.lead.is_some();
    let cutoff = journal::day(now.saturating_sub(PASS_OVER_S)).unwrap_or_default();
    u.files.extend(r.manifest.values().cloned());
    u.records = r.manifest.iter().map(|(l, c)| (l.clone(), c.clone())).collect();
    u.files.extend(r.pending.keys().cloned());
    // (Not records missing an entry the journal's GC removed or removes: a stale read's.)
    crate::pool::journal_gc::fence(&nas, &r)?;
    // (Every day: one before the horizon holds only entries written late, not merged yet.)
    for key in journal::list(&nas, None).context("the journal")? {
        if r.reflected.contains(&key) || r.rejected.contains_key(&key) {
            continue;
        }
        match journal::read(&nas, &key).with_context(|| format!("journal entry {key}"))? {
            journal::Read::Entry(e) => {
                u.entries += 1;
                u.files.extend(e.handoff.pending.keys().cloned());
                for (l, c) in e.handoff.changes {
                    if let Some(c) = c {
                        u.files.insert(c.clone());
                        u.changes.push((l, c));
                    }
                }
            }
            // (The lead decides an entry is lost: it refuses one it can't read whole for an hour
            // awake, and the records then name it. No term led, one `PASS_OVER_S` old is passed.)
            unreadable => {
                let what = match unreadable {
                    journal::Read::Short => "doesn't read whole".to_string(),
                    journal::Read::Damaged(why) => format!("reads damaged ({why})"),
                    _ => "isn't there as read, listed".to_string(),
                };
                let day = key.split('/').next().unwrap_or_default();
                if !led && !cutoff.is_empty() && day < cutoff.as_str() {
                    u.warnings.push(format!(
                        "journal entry {key} {what}, no term led to refuse it, for over {} days: passed over",
                        PASS_OVER_S / 86400
                    ));
                    continue;
                }
                anyhow::bail!("journal entry {key} {what} (not refused yet: nothing removed)");
            }
        }
    }
    Ok(u)
}

/// What GC keeps of `sources/inputs/` (docs/inputs.md §4.9): what the records name (`manifest`,
/// today's file, and with the pool on the newest records' own manifest, which today's may lag:
/// the units' accepted indexes, listings and held reports), the files each such index lists, the
/// same of every journal entry the newest records don't reflect yet (pool.md §7.3: an entry not
/// merged may name a version the records will), the files listed by every index whose time is
/// within `within` of `now` (one made lately, or replaced lately: the check touches the index it
/// replaces), and the index each check's state made within `within` names as replaced, with its
/// files (whatever that index's own time: a touch that failed loses nothing). So a version replaced
/// stays restorable for `within` and a step reading the version it planned with finds its files.
/// Units' names may be nested (`timetables/gtfs`): every folder under `sources/inputs/` is walked.
/// An error when any of them can't be read now: nothing there is swept then.
pub fn inputs_kept(
    root: &store::nasfs::NasRoot,
    manifest: &std::collections::BTreeMap<String, String>,
    um: &Unmerged,
    within: Duration,
    now: SystemTime,
) -> Result<BTreeSet<String>> {
    let mut keep: BTreeSet<String> = BTreeSet::new();
    let ours = |(l, c): &(String, String)| c.starts_with("sources/inputs/").then(|| (l.clone(), c.clone()));
    // Today's manifest, the newest records' and the journal's entries not merged yet (with the
    // pool on: `unmerged`, read once for the whole sweep).
    let mut named: Vec<(String, String)> =
        manifest.iter().map(|(l, c)| (l.clone(), c.clone())).filter_map(|p| ours(&p)).collect();
    named.extend(um.records.iter().filter_map(ours));
    named.extend(um.changes.iter().filter_map(ours));
    let index_files = |c: &str, keep: &mut BTreeSet<String>| -> Result<()> {
        keep.extend(
            crate::inputs::read_index(root, c)?
                .files
                .values()
                .flat_map(|f| f.stored().map(str::to_string))
                .collect::<Vec<_>>(),
        );
        keep.insert(c.to_string());
        Ok(())
    };
    for (l, c) in named {
        if crate::inputs::record_of(&l).is_some_and(|(_, r)| r == "@index") {
            index_files(&c, &mut keep)?;
        }
        keep.insert(c);
    }
    // Every unit's records made within `within` (by walking every folder: units' names nest): an
    // index's files, and a check's state's replaced index with its files.
    let mut stack = vec![root.join("sources/inputs")];
    while let Some(dir) = stack.pop() {
        let rd = match dir.read_dir() {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("list {}", dir.display())),
        };
        for e in rd {
            meta(1, 0, 0);
            if e.file_type()?.is_dir() {
                stack.push(e.path);
                continue;
            }
            let rel = e.path.rel().to_string();
            let Some(record) = store::naming::parse_content_name(&rel)
                .filter(|n| n.ext == "json")
                .and_then(|n| crate::inputs::record_of(n.logical))
                .map(|(_, r)| r)
            else {
                continue;
            };
            meta(0, 1, 0);
            let young =
                e.metadata()?.modified().ok().is_some_and(|t| now.duration_since(t).map_or(true, |d| d <= within));
            if !young {
                continue;
            }
            match record {
                "@index" => index_files(&rel, &mut keep)?,
                "@listed" => {
                    if let Some(old) = crate::inputs::read_listed_file(root, &rel)?.replaced {
                        index_files(&old, &mut keep)?;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(keep)
}

/// The clean-up's step and target, as its costs file names them (`note_cost`; crate::coord::cost_key:
/// `gc gc`, as the lead offers its own job of no targets, the id's rest its target).
pub const STEP: &str = "gc";

/// When this process's run began (`run`), for its cost line's time.
static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Notes what this run cost so far in the job's costs file (`SCENIC_COSTS`, as scenic-build's steps
/// do): its NAS work (crate::costin: bytes read, and the folder entries listed, files statted and
/// deleted, `meta`) over its time, its memory: the clean-up's NAS weight (crate::budget::nas) as the
/// coordinator weighs it from then on, measured, never guessed (docs/pool.md §7.7, task #451).
pub fn note_cost() {
    let Some(p) = std::env::var_os("SCENIC_COSTS") else { return };
    let secs = STARTED.get().map_or(0, |t| t.elapsed().as_secs());
    let line = serde_json::json!({
        "unit": crate::coord::cost_key(STEP, STEP),
        "peak_mb": crate::sys::group_peak() >> 20,
        "secs": secs,
        "v": crate::coord::cost_version(STEP),
        "sizes": crate::costin::take(),
    });
    let r = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
        .and_then(|mut f| std::io::Write::write_all(&mut f, format!("{line}\n").as_bytes()));
    if let Err(e) = r {
        eprintln!("gc: noting what it cost: {e}");
    }
}

/// A safe point between folders: asked to stop (the build pausing, or the agent yielding the NAS
/// to jobs long held back for it, docs/pool.md §7.7), it notes its cost and ends as paused; what it
/// removed is gone for good and the rest waits for its next run (each removal stands alone).
fn yield_point() {
    #[cfg(test)]
    YIELD_CHECKS.set(YIELD_CHECKS.get() + 1);
    if crate::control::draining() {
        note_cost();
        crate::control::stop_paused(STEP);
    }
}

#[cfg(test)]
thread_local! {
    /// The safe points a run on this thread passed (`yield_point`).
    static YIELD_CHECKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

thread_local! {
    /// Whether this run notes its NAS work (`SCENIC_COSTS` set as `run` began: a job's). Its walks
    /// run on the thread that called `run`.
    static COUNTING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Counts NAS metadata work (crate::costin::meta) while this process notes it (`COUNTING`).
fn meta(listed: u64, stats: u64, deletes: u64) {
    if COUNTING.get() {
        crate::costin::meta(listed, stats, deletes);
    }
}

/// `run_once`, its NAS work counted from its start and noted as it ends (`note_cost`), in a job
/// (`SCENIC_COSTS` set).
/// `depth`: the depth's current targets (`DepthKeep`; None: no depth name dropped).
pub fn run(root: &store::nasfs::NasRoot, keep_days: u64, dry_run: bool, depth: Option<&DepthKeep>) -> Result<Report> {
    STARTED.get_or_init(std::time::Instant::now);
    if std::env::var_os("SCENIC_COSTS").is_some() {
        COUNTING.set(true);
        crate::costin::begin();
    }
    // (The names to drop first; a failure is said and tried again next run, never fails GC. Its
    // scratch only a folder: a removal writes nothing.)
    static RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let scratch = std::env::temp_dir().join(format!("scenic-gc-{}-{n}", std::process::id()));
    let dropped = drop_names(root, &scratch, depth, dry_run);
    std::fs::remove_dir_all(&scratch).ok();
    let r = run_once(root, keep_days, dry_run).map(|mut rep| {
        match &dropped {
            Ok((r, d)) => (rep.retired_dropped, rep.depth_dropped) = (*r, *d),
            Err(e) => rep.warnings.push(format!("names not dropped: {e:#}")),
        }
        // The history's old days, one tar a day (task #531); a failure is said, never fails GC.
        match super::histarchive::run(root, crate::agent::jobs::now_s(), dry_run, &yield_point) {
            Ok(h) => {
                meta(h.listed, h.stats, h.files_removed as u64 + h.tmp_removed as u64 + h.days_archived as u64);
                rep.history_days_archived = h.days_archived;
                rep.warnings.extend(h.warnings);
            }
            Err(e) => rep.warnings.push(format!("history not archived: {e:#}")),
        }
        // The pool's journal's old days (task #644); a failure is said, never fails GC.
        // On unless the owner's switch says off (journal_gc::SWITCH).
        let nas = root.clone();
        if !dry_run && root.join(crate::pool::journal::DIR).exists() && crate::pool::journal_gc::enabled(&nas) {
            let rmdir = |d: &str| {
                root.join(d).remove_dir().ok();
            };
            match crate::pool::journal_gc::run(&nas, crate::agent::jobs::now_s(), &yield_point, &rmdir) {
                Ok(j) => {
                    meta(j.listed, 0, j.removed as u64);
                    rep.journal_removed = j.removed;
                    rep.journal_marked = j.marked;
                    rep.warnings.extend(j.waits.into_iter().map(|w| format!("journal: {w}")));
                }
                Err(e) => rep.warnings.push(format!("journal not collected: {e:#}")),
            }
        }
        // The forecasts' archive's old days, one tar a month, kept for good (task #621).
        match super::fcarchive::pack(root, crate::agent::jobs::now_s(), dry_run, &yield_point) {
            Ok(h) => {
                meta(h.listed, h.stats, h.files_removed as u64 + h.tmp_removed as u64);
                rep.forecast_days_archived = h.days_archived;
                rep.warnings.extend(h.warnings);
            }
            Err(e) => rep.warnings.push(format!("forecasts not archived: {e:#}")),
        }
        rep
    });
    note_cost();
    r
}

fn run_once(root: &store::nasfs::NasRoot, keep_days: u64, dry_run: bool) -> Result<Report> {
    use crate::timings::{phase, Class};
    let keep = Duration::from_secs(keep_days * 86400);
    let now = SystemTime::now();
    let old = |t: SystemTime| now.duration_since(t).is_ok_and(|d| d > keep);
    let mut rep = Report { dry_run, ..Default::default() };

    // Catalogues: the newest always, and every one of the last `keep_days`.
    let cat_dir = root.join("catalog");
    // (Phase names are stored in the job logs and their stats: the old spelling stays, task #547.)
    let read = phase("catalogs read", Class::NasRead);
    let ns = if cat_dir.exists() { store::catalogue::list(&cat_dir).context("list catalogues")? } else { Vec::new() };
    // Before the first catalogue there's nothing to keep track of, so nothing is removed.
    let Some(&latest) = ns.iter().max() else { return Ok(rep) };
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    let mut drop_cats: Vec<NasPath> = Vec::new();
    let nas_file = |p: store::nasfs::Place| match p {
        store::nasfs::Place::Nas(p) => Some(p),
        store::nasfs::Place::Local(_) => None,
    };
    // (How far it is, for the status: the catalogues read, then the folders swept.)
    crate::agent::jobs::stage(0, 2, "steps (reading the catalogues)");
    // (Each catalogue's file, whole or a delta: store::catalogue::path_n.)
    let mut in_window: BTreeSet<u64> = BTreeSet::new();
    for &n in &ns {
        let p = store::catalogue::path_n(&cat_dir, n)
            .and_then(nas_file)
            .with_context(|| format!("catalogue {n} gone meanwhile"))?;
        meta(0, 1, 0);
        let mtime = p.metadata().and_then(|m| m.modified()).with_context(|| format!("stat {}", p.display()))?;
        if n == latest || !old(mtime) {
            in_window.insert(n);
        }
    }
    // A delta kept keeps the catalogues it's read through, back to a whole one (their files aren't
    // kept for them: only the catalogues in the window are what's served). One that can't be read
    // stops the sweep.
    let mut kept_cats = in_window.clone();
    for &n in &in_window {
        kept_cats.extend(
            store::catalogue::chain(&cat_dir, n)
                .with_context(|| format!("the catalogues catalogue {n} is read through"))?,
        );
    }
    for &n in ns.iter().filter(|n| !kept_cats.contains(n)) {
        drop_cats.extend(store::catalogue::path_n(&cat_dir, n).and_then(nas_file));
    }
    // Read oldest first, each delta applied to the one before when that's just been read (not
    // its chain read again).
    let mut prev: Option<store::catalogue::Catalogue> = None;
    for (k, &n) in in_window.iter().enumerate() {
        crate::agent::jobs::within(k as f64 / in_window.len() as f64);
        let p = store::catalogue::path_n(&cat_dir, n)
            .and_then(nas_file)
            .with_context(|| format!("catalogue {n} gone meanwhile"))?;
        // A catalogue in the window that can't be read stops the sweep: its files might be in use.
        let delta = (p.file_name() == Some(store::catalogue::delta_file_name(n).as_str()))
            .then(|| p.read().ok().and_then(|b| store::catalogue::Delta::decode(&b).ok()))
            .flatten();
        let cat = match (delta, &prev) {
            (Some(d), Some(c)) if c.n == d.of => d.apply(c).with_context(|| format!("read {}", p.display()))?,
            _ => store::catalogue::read_n(&cat_dir, n).with_context(|| format!("read {}", p.display()))?,
        };
        read.count(p.metadata().map(|m| m.len()).unwrap_or(0), 1);
        referenced.extend(cat.files.values().map(|f| f.file.clone()));
        rep.catalogues_kept += 1;
        prev = Some(cat);
    }
    // The build's manifest: everything a build has uploaded and may publish next (work in flight,
    // and outputs reused by name). Unreadable, nothing is removed.
    // (Not `exists()`: it's false on an I/O error too, and then the manifest's files would go.)
    drop(read);
    let m: std::collections::BTreeMap<String, String> = {
        let _p = phase("build manifest read", Class::NasRead);
        crate::out::read_record(&root.join("state/build/manifest.json")).context("the build manifest")?
    };
    // The pool's newest records and its journal's entries not merged yet: unreadable, nothing
    // removed, the job failed (said in the status, tried again as failures are); entries passed
    // over, no term led, said as warnings (the job's note).
    let um = {
        let _p = phase("the journal read", Class::NasRead);
        let now_s = now.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        unmerged(root, now_s).context("nothing removed: the pool's records or journal can't be read whole")?
    };
    for w in &um.warnings {
        eprintln!("gc: warning: {w}");
    }
    rep.warnings.extend(um.warnings.iter().cloned());
    rep.unmerged = um.entries;
    // (What the gate's copies are kept by, read before the manifest is given up.)
    let inputs_keep = match inputs_kept(root, &m, &um, keep, now) {
        Ok(k) => Some(k),
        Err(e) => {
            rep.inputs_skipped = Some(format!("{e:#}"));
            None
        }
    };
    referenced.extend(m.into_values());
    referenced.extend(um.files);
    rep.referenced = referenced.len();
    let tops: BTreeSet<String> = referenced
        .iter()
        .filter_map(|f| f.split('/').next())
        .filter(|t| !NEVER.contains(t))
        .map(str::to_string)
        .collect();
    // A retired prefix outside `NEVER` adds its own folder only, when its top folder isn't swept
    // anyway (never the rest of that top folder; `sweep_retired_names` takes those under `NEVER`).
    let tops: Vec<String> = RETIRED
        .iter()
        .filter_map(|p| p.rsplit_once('/').map(|(d, _)| d))
        .filter(|d| d.split('/').next().is_some_and(|t| !NEVER.contains(&t) && !tops.contains(t)))
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .chain(tops)
        .collect();

    crate::agent::jobs::stage(1, 2, "steps (sweeping the folders)");
    // (Listing the folders and removing what's due, file by file: one phase, the removals counted.)
    let sweep = phase("folders swept", Class::NasWrite);
    for (k, top) in tops.iter().enumerate() {
        yield_point();
        crate::agent::jobs::within(k as f64 / tops.len() as f64);
        let mut stack = vec![root.join(top)];
        while let Some(dir) = stack.pop() {
            yield_point();
            let Ok(rd) = dir.read_dir() else { continue };
            for e in rd {
                let p = e.path.clone();
                meta(1, 1, 0);
                let Ok(md) = e.metadata() else { continue };
                if md.is_dir() {
                    stack.push(p);
                    continue;
                }
                rep.files_seen += 1;
                if rep.restored.contains(&p) {
                    continue;
                }
                let rel = p.rel().to_string();
                let name = e.name.clone();
                let mtime = md.modified().unwrap_or(now);
                if name.ends_with(TOMB) {
                    put_back(&p, dry_run, &mut rep);
                    continue;
                }
                if name.ends_with(".tmp") {
                    // A write that never finished (two days: no write takes that long).
                    if now.duration_since(mtime).is_ok_and(|d| d > Duration::from_secs(2 * 86400)) {
                        remove(&p, dry_run)?;
                        rep.tmp_removed += 1;
                    }
                    continue;
                }
                if store::naming::parse_content_name(&rel).is_none() || referenced.contains(&rel) {
                    continue;
                }
                // (Its time looked at again right before it goes: a job reusing it by name touches
                // it, out::Out::put_file.)
                // (Gone only if no job reused it by name meanwhile: `remove_unless_touched`.)
                if !old(mtime) || !remove_unless_touched(&p, &old, dry_run)? {
                    rep.young += 1;
                    continue;
                }
                rep.removed += 1;
                rep.removed_bytes += md.len();
                if retired(&rel) {
                    rep.retired_removed += 1;
                    rep.retired_bytes += md.len();
                }
                sweep.count(md.len(), 1);
            }
        }
    }
    drop(sweep);
    // The gate's checked copies: what's kept named, the rest once old.
    if let Some(keep) = inputs_keep {
        let _p = phase("the inputs' copies swept", Class::NasWrite);
        let mut stack = vec![root.join("sources/inputs")];
        while let Some(dir) = stack.pop() {
            yield_point();
            let Ok(rd) = dir.read_dir() else { continue };
            for e in rd {
                let p = e.path.clone();
                meta(1, 1, 0);
                let Ok(md) = e.metadata() else { continue };
                if md.is_dir() {
                    stack.push(p);
                    continue;
                }
                rep.files_seen += 1;
                if rep.restored.contains(&p) {
                    continue;
                }
                let rel = p.rel().to_string();
                let mtime = md.modified().unwrap_or(now);
                if e.name.ends_with(TOMB) {
                    put_back(&p, dry_run, &mut rep);
                    continue;
                }
                if e.name.ends_with(".tmp") {
                    if now.duration_since(mtime).is_ok_and(|d| d > Duration::from_secs(2 * 86400)) {
                        remove(&p, dry_run)?;
                        rep.tmp_removed += 1;
                    }
                    continue;
                }
                if store::naming::parse_content_name(&rel).is_none() || keep.contains(&rel) || referenced.contains(&rel)
                {
                    continue;
                }
                if !old(mtime) || !remove_unless_touched(&p, &old, dry_run)? {
                    rep.young += 1;
                    continue;
                }
                crate::timings::count(md.len(), 1);
                rep.removed += 1;
                rep.removed_bytes += md.len();
                rep.inputs_removed += 1;
            }
        }
    }
    // Retired passes' sources: passes older than the newest complete one.
    if let Some(latest) = crate::osmpass::latest_pass(root) {
        let _p = phase("retired passes swept", Class::NasWrite);
        // When the newest pass completed (its summary's time): a retired pass's plain files (the
        // planet download) go once that's `keep_days` ago, so a fresh pass can still be compared.
        let done_at = root
            .join("sources/osm")
            .join(&latest)
            .read_dir()
            .ok()
            .and_then(|rd| rd.into_iter().find(|e| e.name.starts_with("pass.") && e.name.ends_with(".json")))
            .and_then(|e| e.metadata().ok()?.modified().ok());
        let plain_due = done_at.is_some_and(old);
        for kind in ["osm", "items"] {
            let Ok(rd) = root.join("sources").join(kind).read_dir() else { continue };
            let mut dates: Vec<String> =
                rd.into_iter().map(|e| e.name).filter(|d| crate::osmpass::is_date(d) && *d < latest).collect();
            dates.sort();
            for d in dates {
                yield_point();
                let dir = root.join("sources").join(kind).join(&d);
                sweep_retired(&dir, &referenced, &old, plain_due, dry_run, &mut rep)?;
            }
        }
    }
    {
        let _p = phase("removed steps' files swept", Class::NasWrite);
        sweep_retired_names(root, &referenced, &old, dry_run, &mut rep)?;
    }
    // (Stored phase name: old spelling, task #547.)
    let _p = phase("old catalogs removed", Class::NasWrite);
    for p in drop_cats {
        remove(&p, dry_run)?;
        rep.catalogues_removed += 1;
    }
    Ok(rep)
}

/// One retired pass's folder: content-named files no catalogue or manifest names, once old; any other
/// file once `plain_due`; then the folders left empty.
fn sweep_retired(
    dir: &NasPath,
    referenced: &BTreeSet<String>,
    old: &dyn Fn(SystemTime) -> bool,
    plain_due: bool,
    dry_run: bool,
    rep: &mut Report,
) -> Result<()> {
    let mut dirs = vec![dir.clone()];
    let mut stack = vec![dir.clone()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = d.read_dir() else { continue };
        for e in rd {
            let p = e.path.clone();
            meta(1, 1, 0);
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                stack.push(p.clone());
                dirs.push(p);
                continue;
            }
            rep.files_seen += 1;
            if e.name.ends_with(TOMB) {
                put_back(&p, dry_run, rep);
                continue;
            }
            if rep.restored.contains(&p) {
                continue;
            }
            let rel = p.rel().to_string();
            let named = store::naming::parse_content_name(&rel).is_some();
            let due = if named {
                !referenced.contains(&rel)
                    && old(md.modified().unwrap_or(SystemTime::now()))
                    && remove_unless_touched(&p, old, dry_run)?
            } else if plain_due {
                remove(&p, dry_run)?;
                true
            } else {
                false
            };
            if due {
                crate::timings::count(md.len(), 1);
                rep.removed += 1;
                rep.removed_bytes += md.len();
                rep.retired_removed += 1;
                rep.retired_bytes += md.len();
            } else if named && !referenced.contains(&rel) {
                rep.young += 1;
            }
        }
    }
    // Deepest first: a folder emptied above may let its parent go too.
    dirs.sort_by_key(|d| std::cmp::Reverse(d.rel().split('/').count()));
    for d in dirs {
        meta(1, 0, 0);
        if d.read_dir().is_ok_and(|rd| rd.is_empty()) && !dry_run {
            meta(0, 0, 1);
            d.remove_dir().ok();
        }
    }
    Ok(())
}

/// The files of removed steps (`RETIRED`) under `sources/` (`NEVER` keeps the folder sweep out; a
/// prefix elsewhere is the folder sweep's), in each prefix's folder (not below it): content-named
/// ones no catalogue, manifest or records name, once old; a tombstone of one put back.
fn sweep_retired_names(
    root: &store::nasfs::NasRoot,
    referenced: &BTreeSet<String>,
    old: &dyn Fn(SystemTime) -> bool,
    dry_run: bool,
    rep: &mut Report,
) -> Result<()> {
    for prefix in RETIRED.iter().filter(|p| p.split('/').next().is_some_and(|t| NEVER.contains(&t))) {
        yield_point();
        let dir = prefix.rsplit_once('/').map_or("", |(d, _)| d);
        let Ok(rd) = root.join(dir).read_dir() else { continue };
        for e in rd {
            let rel = e.path.rel().to_string();
            meta(1, 1, 0);
            if !rel.starts_with(prefix) || rep.restored.contains(&e.path) {
                continue;
            }
            if e.name.ends_with(TOMB) {
                put_back(&e.path, dry_run, rep);
                continue;
            }
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() || store::naming::parse_content_name(&rel).is_none() || referenced.contains(&rel) {
                continue;
            }
            rep.files_seen += 1;
            if !old(md.modified().unwrap_or(SystemTime::now())) || !remove_unless_touched(&e.path, old, dry_run)? {
                rep.young += 1;
                continue;
            }
            crate::timings::count(md.len(), 1);
            rep.removed += 1;
            rep.removed_bytes += md.len();
            rep.retired_removed += 1;
            rep.retired_bytes += md.len();
        }
    }
    Ok(())
}

/// Whether file `p` is still old by a fresh look at its time (not the listing's: a job reusing a
/// content-named file touches it, and GC's listing may be minutes old by the time it gets there).
fn still_old(p: &NasPath, old: &dyn Fn(SystemTime) -> bool) -> bool {
    meta(0, 1, 0);
    p.metadata().ok().and_then(|m| m.modified().ok()).is_some_and(old)
}

/// What a file GC is about to remove is renamed to first (`remove_unless_touched`).
const TOMB: &str = store::naming::TOMB;

/// A tombstone a sweep finds (left by a GC stopped between its rename and its removal, a put-back
/// that failed, or another GC's still in flight): never removed here, but put back under its name
/// when that's free, for the usual rules to decide next run; when its name has a file again (a job
/// uploaded the same bytes meanwhile), the tombstone's copy is the same bytes and goes. Another
/// GC's in flight finds its tombstone gone and keeps the file (`remove_unless_touched`).
fn put_back(tomb: &NasPath, dry_run: bool, rep: &mut Report) {
    let Some(name) = tomb.file_name().and_then(|t| t.strip_suffix(TOMB)).map(|n| tomb.with_file_name(n)) else {
        return;
    };
    if dry_run {
        eprintln!("gc: would put {} back", name.display());
        return;
    }
    // (`restore`: never over a file of its name a job has uploaded again.)
    match restore(tomb, &name) {
        Ok(()) => {
            rep.tombs += 1;
            rep.restored.insert(name);
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => eprintln!("gc: putting {} back: {e}", name.display()),
    }
}

/// Tombstone `tomb` back under `name`, by a rename that never replaces: a file there already (a job
/// uploaded the same bytes again meanwhile, newer than the tombstone's copy) is kept, and the
/// tombstone goes. (A replacing rename would put the tombstone's old time over the new upload's,
/// for the next run to remove.)
fn restore(tomb: &NasPath, name: &NasPath) -> std::io::Result<()> {
    meta(0, 0, 1);
    match tomb.rename_no_replace(name) {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => tomb.remove_file(),
        r => r,
    }
}

/// Removes content-named file `p`, unless a job reused it by name meanwhile: renamed to a tombstone
/// first (a job reusing it now finds no file of its name, and uploads it again: out::Out::put_file),
/// its time then looked at again, the tombstone's: touched since GC listed it (a job had it open
/// before the rename), put back; else removed. True when it went.
fn remove_unless_touched(p: &NasPath, old: &dyn Fn(SystemTime) -> bool, dry_run: bool) -> Result<bool> {
    if dry_run {
        eprintln!("gc: would remove {}", p.display());
        return Ok(still_old(p, old));
    }
    let tomb = p.with_file_name(format!("{}{TOMB}", p.file_name().unwrap_or_default()));
    // (Not renamed aside, for whatever reason: kept this run, as a young file is.)
    // (A rename and a removal: each a metadata write, counted as a delete.)
    meta(0, 0, 1);
    if let Err(e) = p.rename(&tomb) {
        if e.kind() != std::io::ErrorKind::NotFound {
            eprintln!("gc: {} not set aside ({e}): kept this run", p.display());
        }
        return Ok(false);
    }
    if still_old(&tomb, old) {
        meta(0, 0, 1);
        return match tomb.remove_file() {
            Ok(()) => Ok(true),
            // (Another GC put it back meanwhile: kept.)
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e).with_context(|| format!("remove {}", tomb.display())),
        };
    }
    // (Touched: back under its name; one uploaded again meanwhile is the same bytes, its name its
    // content's, and kept: `restore`. Gone already, another GC put it back: kept. Failing, the
    // next run puts it back.)
    if let Err(e) = restore(&tomb, p) {
        if e.kind() != std::io::ErrorKind::NotFound {
            eprintln!("gc: putting {} back: {e} (the next run does)", p.display());
        }
    }
    Ok(false)
}

fn remove(p: &NasPath, dry_run: bool) -> Result<()> {
    if dry_run {
        eprintln!("gc: would remove {}", p.display());
        return Ok(());
    }
    meta(0, 0, 1);
    p.remove_file().with_context(|| format!("remove {}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, FileTimes};
    use std::path::{Path, PathBuf};

    fn age(p: &Path, days: u64) {
        let t = SystemTime::now() - Duration::from_secs(days * 86400);
        fs::File::options().write(true).open(p).unwrap().set_times(FileTimes::new().set_modified(t)).unwrap();
    }

    #[test]
    fn keeps_referenced_and_young_removes_the_rest() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let put = |rel: &str, days: u64| {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, b"x").unwrap();
            age(&p, days);
        };
        let kept = "layers/roads/hi/6-1-2.0123456789abcdef.pack";
        put(kept, 30);
        put("layers/roads/hi/6-1-2.fedcba9876543210.pack", 30); // replaced, old: removed
        put("layers/roads/hi/6-1-3.1111111111111111.pack", 1); // unreferenced but young: kept
        put("layers/roads/hi/6-1-4.2222222222222222.pack.tmp", 5); // abandoned write: removed
        put("sources/osm/x.3333333333333333.osm.pbf", 300); // never swept
        put("layers/roads/README", 300); // not content-named: kept
        let mut cat = store::catalogue::Catalogue::new(1);
        cat.files.insert(
            "layers/roads/hi/6-1-2".into(),
            store::catalogue::FileRef { file: kept.into(), ..Default::default() },
        );
        store::catalogue::write(&root.join("catalog"), &cat).unwrap();
        let r = run(&store::nasfs::NasRoot::local_for_test(root), 14, false, None).unwrap();
        assert_eq!((r.removed, r.tmp_removed, r.young), (1, 1, 1));
        assert!(root.join(kept).exists());
        assert!(!root.join("layers/roads/hi/6-1-2.fedcba9876543210.pack").exists());
        assert!(root.join("layers/roads/hi/6-1-3.1111111111111111.pack").exists());
        assert!(root.join("sources/osm/x.3333333333333333.osm.pbf").exists());
        assert!(root.join("layers/roads/README").exists());
    }

    /// Task #451: a run in a job counts its NAS metadata work (crate::costin: `listed`, `stats`,
    /// `deletes`), its weight beside its bytes; outside one (no `SCENIC_COSTS`) nothing.
    #[test]
    fn a_run_counts_its_listings_stats_and_deletes() {
        let _l = crate::costin::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        for (rel, days) in [("layers/a/6-1-2.0123456789abcdef.pack", 30), ("layers/a/6-1-3.fedcba9876543210.pack", 30)]
        {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, b"x").unwrap();
            age(&p, days);
        }
        let mut cat = store::catalogue::Catalogue::new(1);
        cat.files.insert(
            "layers/a/6-1-2".into(),
            store::catalogue::FileRef { file: "layers/a/6-1-2.0123456789abcdef.pack".into(), ..Default::default() },
        );
        store::catalogue::write(&root.join("catalog"), &cat).unwrap();
        crate::costin::begin();
        COUNTING.set(true);
        let r = run_once(&store::nasfs::NasRoot::local_for_test(root), 14, false).unwrap();
        COUNTING.set(false);
        let m = crate::costin::take();
        assert_eq!(r.removed, 1);
        assert!(m["listed"] >= 2 && m["stats"] >= 3, "{m:?}");
        // (Its rename aside and its removal.)
        assert_eq!(m["deletes"], 2, "{m:?}");
        crate::costin::begin();
        meta(5, 5, 5);
        assert!(!crate::costin::take().contains_key("listed"), "not in a job: not counted");
    }

    /// Task #451 review: the main sweep passes a safe point at every folder, not only at each
    /// top folder, so a yield waits for one folder's work at most.
    #[test]
    fn the_sweep_passes_a_safe_point_each_folder() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let mut cat = store::catalogue::Catalogue::new(1);
        for i in 0..6 {
            let rel = format!("layers/a/d{i}/6-1-{i}.0123456789abcdef.pack");
            fs::create_dir_all(root.join(&rel).parent().unwrap()).unwrap();
            fs::write(root.join(&rel), b"x").unwrap();
            cat.files.insert(
                format!("layers/a/d{i}/6-1-{i}"),
                store::catalogue::FileRef { file: rel, ..Default::default() },
            );
        }
        store::catalogue::write(&root.join("catalog"), &cat).unwrap();
        YIELD_CHECKS.set(0);
        run(&store::nasfs::NasRoot::local_for_test(root), 14, false, None).unwrap();
        // (layers, layers/a and its six folders.)
        assert!(YIELD_CHECKS.get() >= 8, "{}", YIELD_CHECKS.get());
    }

    #[test]
    fn keeps_the_newest_catalogue_however_old() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let mk = |n: u64, file: &str| {
            let p = root.join(file);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, b"x").unwrap();
            age(&p, 60);
            let mut cat = store::catalogue::Catalogue::new(n);
            let logical = file.rsplit_once('.').unwrap().0.rsplit_once('.').unwrap().0;
            cat.files.insert(logical.into(), store::catalogue::FileRef { file: file.into(), ..Default::default() });
            let c = store::catalogue::write(&root.join("catalog"), &cat).unwrap();
            age(&c, 60);
        };
        mk(1, "layers/a/hi/6-1-1.1111111111111111.pack");
        mk(2, "layers/a/hi/6-1-1.2222222222222222.pack");
        mk(3, "layers/a/hi/6-1-1.3333333333333333.pack");
        let r = run(&store::nasfs::NasRoot::local_for_test(root), 14, false, None).unwrap();
        assert_eq!((r.catalogues_kept, r.catalogues_removed, r.removed), (1, 2, 2));
        assert!(root.join("layers/a/hi/6-1-1.3333333333333333.pack").exists());
        assert_eq!(store::catalogue::latest(&root.join("catalog")).unwrap().unwrap().n, 3);
        assert_eq!(store::catalogue::next_n(&root.join("catalog")).unwrap(), 4);
    }

    #[test]
    fn a_delta_kept_keeps_the_catalogues_its_read_through_and_its_files() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let dir = root.join("catalog");
        let mut prev: Option<store::catalogue::Catalogue> = None;
        // 1 whole and old, 2 a delta and old, 3 and 4 deltas in the window (3 over 2, 4 over 3).
        for (n, old) in [(1, true), (2, true), (3, false), (4, false)] {
            let file = format!("layers/a/hi/6-1-{n}.{}.pack", format!("{n}").repeat(16));
            let p = root.join(&file);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, b"x").unwrap();
            age(&p, 60);
            let mut cat = prev.clone().unwrap_or_else(|| store::catalogue::Catalogue::new(1));
            cat.n = n;
            cat.files.insert(format!("layers/a/hi/6-1-{n}"), store::catalogue::FileRef { file, ..Default::default() });
            let c = match &prev {
                None => store::catalogue::write(&dir, &cat).unwrap(),
                Some(b) => store::catalogue::write_delta(&dir, b, &cat).unwrap(),
            };
            if old {
                age(&c, 60);
            }
            prev = Some(cat);
        }
        let r = run(&store::nasfs::NasRoot::local_for_test(root), 14, false, None).unwrap();
        // (3 and 4 kept, read; 1 and 2 kept as what 3 is read through, their files not kept for them
        // but 3's names them all.)
        assert_eq!((r.catalogues_kept, r.catalogues_removed, r.removed), (2, 0, 0));
        assert_eq!(store::catalogue::latest(&dir).unwrap().unwrap(), prev.unwrap());
        assert_eq!(store::catalogue::list(&dir).unwrap(), [4, 3, 2, 1]);
    }

    #[test]
    fn the_build_manifest_keeps_its_files() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let f = "base/6-1-1.4444444444444444.sect";
        fs::create_dir_all(root.join("base")).unwrap();
        fs::write(root.join(f), b"x").unwrap();
        age(&root.join(f), 60);
        let cat = store::catalogue::Catalogue::new(1);
        store::catalogue::write(&root.join("catalog"), &cat).unwrap();
        // Not in the catalogue, old, but the build uploaded it: kept. (A catalogue file elsewhere so
        // `base/` is swept at all.)
        let mut cat2 = store::catalogue::Catalogue::new(2);
        cat2.files.insert(
            "base/6-1-2".into(),
            store::catalogue::FileRef { file: "base/6-1-2.5555555555555555.sect".into(), ..Default::default() },
        );
        store::catalogue::write(&root.join("catalog"), &cat2).unwrap();
        fs::create_dir_all(root.join("state/build")).unwrap();
        fs::write(root.join("state/build/manifest.json"), format!("{{\"base/6-1-1\": \"{f}\"}}")).unwrap();
        let r = run(&store::nasfs::NasRoot::local_for_test(root), 14, false, None).unwrap();
        assert_eq!(r.removed, 0);
        assert!(root.join(f).exists());
    }

    #[test]
    fn retired_passes_go_after_the_newer_one_settles() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let put = |rel: &str, days: u64| {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, b"x").unwrap();
            age(&p, days);
        };
        // The old pass (retired from the manifest), the newest (complete 20 days ago), and a planet
        // waiting for its pass.
        put("sources/osm/2026-03-01/planet.osm.pbf", 200);
        put("sources/osm/2026-03-01/pieces/6-1-1.1111111111111111.osm.pbf", 200);
        put("sources/items/2026-03-01/facts.2222222222222222.json", 200);
        put("sources/osm/2026-09-28/planet.osm.pbf", 30);
        put("sources/osm/2026-09-28/pass.3333333333333333.json", 20);
        put("sources/osm/2026-09-28/pieces/6-1-1.4444444444444444.osm.pbf", 25);
        put("sources/osm/2027-03-01/planet.osm.pbf", 1);
        put("sources/registers/legacy.5555555555555555.tar.zst", 300);
        store::catalogue::write(&root.join("catalog"), &store::catalogue::Catalogue::new(1)).unwrap();
        let r = run(&store::nasfs::NasRoot::local_for_test(root), 14, false, None).unwrap();
        assert_eq!(r.retired_removed, 3, "{r:?}");
        assert!(
            !root.join("sources/osm/2026-03-01").exists() && !root.join("sources/items/2026-03-01").exists(),
            "the retired pass's folders, emptied, go too"
        );
        for kept in [
            "sources/osm/2026-09-28/planet.osm.pbf",
            "sources/osm/2026-09-28/pieces/6-1-1.4444444444444444.osm.pbf",
            "sources/osm/2027-03-01/planet.osm.pbf",
            "sources/registers/legacy.5555555555555555.tar.zst",
        ] {
            assert!(root.join(kept).exists(), "{kept}");
        }
        // A newer pass completed only days ago: the old planet waits (its content-named files don't).
        put("sources/osm/2026-03-01/planet.osm.pbf", 200);
        put("sources/osm/2026-03-01/roads/6-1-1.6666666666666666.bin", 200);
        put("sources/osm/2026-09-28/pass.3333333333333333.json", 3);
        let r = run(&store::nasfs::NasRoot::local_for_test(root), 14, false, None).unwrap();
        assert_eq!(r.retired_removed, 1);
        assert!(root.join("sources/osm/2026-03-01/planet.osm.pbf").exists());
    }

    /// The gate's copies (docs/inputs.md §4.9): kept while the manifest's index or report names
    /// them, or a journal entry not merged yet does; the rest of `sources/` and the inputs folders
    /// never swept; none swept while an index can't be read.
    #[test]
    fn the_inputs_copies_go_once_nothing_names_them() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let put = |rel: &str, body: &str, days: u64| {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, body).unwrap();
            age(&p, days);
        };
        let u = "sources/inputs/_gate-test";
        let kept = format!("{u}/a.1111111111111111.jsonl");
        let index = |unit: &str, files: &[&str]| {
            let files: serde_json::Map<String, serde_json::Value> = files
                .iter()
                .enumerate()
                .map(|(i, f)| (format!("f{i}.jsonl"), serde_json::json!({"file": f, "size": 1, "keyed": "-"})))
                .collect();
            serde_json::json!({"fmt": 1, "unit": unit, "checks": format!("{unit} 1"), "files": files}).to_string()
        };
        put(&kept, "x", 60);
        put(&format!("{u}/a.2222222222222222.jsonl"), "x", 60); // an old version: goes
        put(&format!("{u}/a.3333333333333333.jsonl"), "x", 3); // young: stays
        put(&format!("{u}/b.4444444444444444.jsonl"), "x", 60); // an unmerged entry's index lists it
        put(&format!("{u}/@index.5555555555555555.json"), &index("_gate-test", &[&kept]), 60);
        put(
            &format!("{u}/@index.6666666666666666.json"),
            &index("_gate-test", &[&format!("{u}/b.4444444444444444.jsonl")]),
            60,
        );
        put(&format!("{u}/@held.7777777777777777.json"), "{}", 60);
        put(&format!("{u}/@held.8888888888888888.json"), "{}", 60); // an old report: goes
        put("inputs/_gate-test/a.jsonl", "x", 300); // the inputs folder: never
        put("sources/registers/legacy.9999999999999999.tar.zst", "x", 300); // the rest of sources/: never
        store::catalogue::write(&root.join("catalog"), &store::catalogue::Catalogue::new(1)).unwrap();
        fs::create_dir_all(root.join("state/build")).unwrap();
        // A nested unit (`timetables/gtfs`'s kind): its index named, its copy kept; an old copy of
        // it gone.
        let n = "sources/inputs/_gate-nest/inner";
        put(&format!("{n}/c.aaaaaaaaaaaaaaab.zip"), "x", 60);
        put(&format!("{n}/c.aaaaaaaaaaaaaaac.zip"), "x", 60); // an old version: goes
        put(
            &format!("{n}/@index.aaaaaaaaaaaaaaad.json"),
            &index("_gate-nest/inner", &[&format!("{n}/c.aaaaaaaaaaaaaaab.zip")]),
            60,
        );
        fs::write(root.join("state/build/manifest.json"), serde_json::json!({format!("{u}/@index"): format!("{u}/@index.5555555555555555.json"), format!("{u}/@held"): format!("{u}/@held.7777777777777777.json"), format!("{n}/@index"): format!("{n}/@index.aaaaaaaaaaaaaaad.json")}).to_string()).unwrap();
        // A journal entry the records don't reflect, naming the newer index.
        let entry = crate::pool::journal::Entry {
            member: "m-0000000000000001".into(),
            lease: crate::pool::journal::LeaseId { term: 1, n: 7 },
            step: "inputs".into(),
            handoff: crate::handoff::Handoff {
                changes: [(format!("{u}/@index"), Some(format!("{u}/@index.6666666666666666.json")))].into(),
                ..Default::default()
            },
            at: 1_791_500_000,
        };
        crate::pool::journal::write(&store::nasfs::NasRoot::local_for_test(&root), &entry).unwrap();
        let r = run(&store::nasfs::NasRoot::local_for_test(root), 14, false, None).unwrap();
        assert_eq!(r.inputs_skipped, None);
        assert_eq!(r.inputs_removed, 3, "{r:?}");
        assert!(
            !root.join(format!("{u}/a.2222222222222222.jsonl")).exists()
                && !root.join(format!("{u}/@held.8888888888888888.json")).exists()
                && !root.join(format!("{n}/c.aaaaaaaaaaaaaaac.zip")).exists()
        );
        for k in [
            kept.clone(),
            format!("{u}/a.3333333333333333.jsonl"),
            format!("{u}/b.4444444444444444.jsonl"),
            format!("{u}/@index.5555555555555555.json"),
            format!("{u}/@index.6666666666666666.json"),
            format!("{u}/@held.7777777777777777.json"),
            format!("{n}/c.aaaaaaaaaaaaaaab.zip"),
            "inputs/_gate-test/a.jsonl".into(),
            "sources/registers/legacy.9999999999999999.tar.zst".into(),
        ] {
            assert!(root.join(&k).exists(), "{k}");
        }
        // A version replaced: its index touched as the check replaced it (yesterday), the copy it
        // lists accepted 30 days ago and named by no record any more: kept, restorable.
        put(&format!("{u}/a.cccccccccccccccc.jsonl"), "x", 30);
        put(
            &format!("{u}/@index.dddddddddddddddd.json"),
            &index("_gate-test", &[&format!("{u}/a.cccccccccccccccc.jsonl")]),
            1,
        );
        let r = run(&store::nasfs::NasRoot::local_for_test(root), 14, false, None).unwrap();
        assert_eq!(r.inputs_removed, 0, "{r:?}");
        assert!(root.join(format!("{u}/a.cccccccccccccccc.jsonl")).exists());
        // Its touch failed (its time 30 days old), but the check's state made yesterday names it
        // replaced: kept all the same, the nested unit's alike.
        age(&root.join(format!("{u}/@index.dddddddddddddddd.json")), 30);
        put(&format!("{u}/@listed.eeeeeeeeeeeeeeee.json"), &serde_json::json!({"fmt": 1, "unit": "_gate-test", "listed": {}, "replaced": format!("{u}/@index.dddddddddddddddd.json")}).to_string(), 1);
        put(&format!("{n}/c.aaaaaaaaaaaaaaae.zip"), "x", 40);
        put(
            &format!("{n}/@index.aaaaaaaaaaaaaaaf.json"),
            &index("_gate-nest/inner", &[&format!("{n}/c.aaaaaaaaaaaaaaae.zip")]),
            40,
        );
        put(&format!("{n}/@listed.bbbbbbbbbbbbbbbb.json"), &serde_json::json!({"fmt": 1, "unit": "_gate-nest/inner", "listed": {}, "replaced": format!("{n}/@index.aaaaaaaaaaaaaaaf.json")}).to_string(), 2);
        let r = run(&store::nasfs::NasRoot::local_for_test(root), 14, false, None).unwrap();
        assert_eq!(r.inputs_removed, 0, "{r:?}");
        assert!(
            root.join(format!("{u}/a.cccccccccccccccc.jsonl")).exists()
                && root.join(format!("{n}/c.aaaaaaaaaaaaaaae.zip")).exists()
        );
        // 15 days after it was replaced (the state that named it that old too): gone, with its index.
        age(&root.join(format!("{u}/@listed.eeeeeeeeeeeeeeee.json")), 15);
        let r = run(&store::nasfs::NasRoot::local_for_test(root), 14, false, None).unwrap();
        assert_eq!(r.inputs_removed, 3, "the copy, its index, the old state: {r:?}");
        assert!(!root.join(format!("{u}/a.cccccccccccccccc.jsonl")).exists());
        // An index named that can't be read: nothing there swept.
        put(&format!("{u}/a.aaaaaaaaaaaaaaaa.jsonl"), "x", 60);
        fs::write(root.join(format!("{u}/@index.5555555555555555.json")), b"{not json").unwrap();
        let r = run(&store::nasfs::NasRoot::local_for_test(root), 14, false, None).unwrap();
        assert!(r.inputs_skipped.is_some() && r.inputs_removed == 0);
        assert!(root.join(format!("{u}/a.aaaaaaaaaaaaaaaa.jsonl")).exists());
    }

    /// A journal entry the newest records don't reflect yet keeps the files it names (its changes
    /// and its uploads), however old; once merged (or refused) it keeps nothing of its own; an
    /// entry not whole yet keeps GC from removing anything, until refused, or with no term led,
    /// `PASS_OVER_S` old.
    #[test]
    fn an_unmerged_entry_keeps_its_files() {
        use crate::pool::journal::{self, Entry, LeaseId};
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let put = |rel: &str, days: u64| {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, b"x").unwrap();
            age(&p, days);
        };
        let (a, b, c) = (
            "layers/roads/hi/6-1-2.1111111111111111.pack",
            "layers/roads/hi/6-1-3.2222222222222222.pack",
            "layers/roads/hi/6-1-4.3333333333333333.pack",
        );
        for f in [a, b, c] {
            put(f, 60);
        }
        let mut cat = store::catalogue::Catalogue::new(1);
        cat.files.insert(
            "layers/roads/hi/6-9-9".into(),
            store::catalogue::FileRef {
                file: "layers/roads/hi/6-9-9.9999999999999999.pack".into(),
                ..Default::default()
            },
        );
        store::catalogue::write(&root.join("catalog"), &cat).unwrap();
        fs::create_dir_all(root.join("state/build")).unwrap();
        fs::write(root.join("state/build/manifest.json"), "{}").unwrap();
        // An entry of a member away three weeks, written, never merged: its change and its upload.
        let nas = store::nasfs::NasRoot::local_for_test(&root);
        let mut h = crate::handoff::Handoff::default();
        h.changes.insert("layers/roads/hi/6-1-2".into(), Some(a.into()));
        h.pending.insert(b.into(), "0".repeat(64));
        let e = Entry {
            member: "m-0000000000000002".into(),
            lease: LeaseId { term: 1, n: 4 },
            step: "pack".into(),
            handoff: h,
            at: 1_791_500_000,
        };
        journal::write(&nas, &e).unwrap();
        let r = run(&store::nasfs::NasRoot::local_for_test(root), 14, false, None).unwrap();
        assert_eq!((r.unmerged, r.removed), (1, 1), "{r:?}");
        assert!(root.join(a).exists() && root.join(b).exists() && !root.join(c).exists());
        // Merged (term 1's records reflect it, naming nothing of it: replaced since): its files go.
        let me = crate::pool::Member { id: "m-0000000000000001".into(), host: "a".into(), app: "development".into() };
        fs::write(root.join("state/build/jobs.json"), "{}").unwrap();
        fs::write(root.join("state/build/pending.json"), "{}").unwrap();
        crate::pool::term::bootstrap(&nas, &me, 1_791_500_000, false).unwrap().unwrap();
        let mut rec = crate::pool::records::Records { term: 1, ..Default::default() };
        rec.reflected.insert(e.key().unwrap());
        rec.save(&nas).unwrap();
        let r = run(&store::nasfs::NasRoot::local_for_test(root), 14, false, None).unwrap();
        assert_eq!((r.unmerged, r.removed), (0, 2), "{r:?}");
        // A term led: an entry that doesn't read whole (short, of whatever day: a read cut short
        // says nothing of it), or reads damaged: GC fails, nothing removed, every run, until the
        // lead refuses it.
        put(c, 60);
        let now = crate::agent::jobs::now_s();
        let entry_at = |key: &str, bytes: &[u8]| {
            let p = root.join(journal::DIR).join(format!("{key}.json"));
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, bytes).unwrap();
            p
        };
        let short_key = format!("{}/1-5", journal::day(now - 10 * 86400).unwrap());
        entry_at(&short_key, b"{\"member\": ");
        for _ in 0..2 {
            let e = format!("{:#}", run(&store::nasfs::NasRoot::local_for_test(root), 14, false, None).unwrap_err());
            assert!(e.contains("nothing removed") && e.contains("1-5"), "{e}");
            assert!(root.join(c).exists());
        }
        let damaged_key = format!("{}/1-6", journal::day(now).unwrap());
        entry_at(&damaged_key, b"{\"not\": \"an entry\"}");
        rec.rejected.insert(short_key, "never whole".into());
        rec.save(&nas).unwrap();
        let e = format!("{:#}", run(&store::nasfs::NasRoot::local_for_test(root), 14, false, None).unwrap_err());
        assert!(e.contains("damaged") && root.join(c).exists(), "{e}");
        // Both refused by the lead: passed over, the sweep goes on.
        rec.rejected.insert(damaged_key, "damaged".into());
        rec.save(&nas).unwrap();
        let r = run(&store::nasfs::NasRoot::local_for_test(root), 14, false, None).unwrap();
        // (The journal's GC marks the old entries, refused or merged: its own note, said.)
        assert!(r.removed == 1 && r.warnings.iter().all(|w| w.starts_with("journal: marked")), "{r:?}");
        // No term led (its term not readable whole): no one refuses. An entry unreadable from today
        // still fails GC; one whose day is over `PASS_OVER_S` past (ten days here, the days far
        // from the bound) is passed over, said as a warning, the sweep going on.
        fs::write(root.join(crate::pool::term::DIR).join("1.json"), b"{\"term\": ").unwrap();
        put(c, 60);
        let young = entry_at(&format!("{}/1-8", journal::day(now).unwrap()), b"{\"member\": ");
        let e = format!("{:#}", run(&store::nasfs::NasRoot::local_for_test(root), 14, false, None).unwrap_err());
        assert!(e.contains("1-8") && root.join(c).exists(), "{e}");
        fs::remove_file(&young).unwrap();
        entry_at(&format!("{}/1-7", journal::day(now - 10 * 86400).unwrap()), b"{\"member\": ");
        let mut r = run(&store::nasfs::NasRoot::local_for_test(root), 14, false, None).unwrap();
        // (The journal's GC waits for a lead: its own note.)
        r.warnings.retain(|w| !w.starts_with("journal: "));
        assert!(
            r.removed == 1
                && r.warnings.len() == 1
                && r.warnings[0].contains("1-7")
                && r.warnings[0].contains("passed over"),
            "{r:?}"
        );
        assert!(!root.join(c).exists());
    }

    /// A file reused by name as GC removes it: renamed to a tombstone first, its time looked at
    /// again there; touched since GC listed it, put back.
    #[test]
    fn a_file_touched_as_gc_removes_it_is_put_back() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("layers/roads/hi/6-1-2.0123456789abcdef.pack");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, b"x").unwrap();
        let keep = Duration::from_secs(14 * 86400);
        let now = SystemTime::now();
        let old = |t: SystemTime| now.duration_since(t).is_ok_and(|d| d > keep);
        // (Listed old; touched by a job before GC got to it.)
        age(&p, 30);
        fs::File::options().write(true).open(&p).unwrap().set_modified(SystemTime::now()).unwrap();
        assert!(!remove_unless_touched(&store::nasfs::NasPath::local_for_test(&p), &old, false).unwrap());
        assert!(
            p.exists() && fs::read_dir(p.parent().unwrap()).unwrap().count() == 1,
            "back under its name, no tombstone left"
        );
        // Not touched: gone, no tombstone left.
        age(&p, 30);
        assert!(remove_unless_touched(&store::nasfs::NasPath::local_for_test(&p), &old, false).unwrap());
        assert!(!p.exists() && fs::read_dir(p.parent().unwrap()).unwrap().count() == 0);
        // Gone already (another GC, or never there): nothing.
        assert!(!remove_unless_touched(&store::nasfs::NasPath::local_for_test(&p), &old, false).unwrap());
        // A tombstone a sweep finds (a GC stopped between, or another's in flight): never removed
        // as a stale file, put back under its name for the usual rules; its name taken again (the
        // same bytes uploaded meanwhile), it goes. Not a content-named file either way.
        let root = d.path();
        let mut tomb = p.as_os_str().to_owned();
        tomb.push(TOMB);
        let tomb = PathBuf::from(tomb);
        fs::write(&tomb, b"x").unwrap();
        age(&tomb, 30);
        assert!(store::naming::parse_content_name(&tomb.strip_prefix(root).unwrap().to_string_lossy()).is_none());
        let mut cat = store::catalogue::Catalogue::new(1);
        cat.files.insert(
            "layers/roads/hi/6-9-9".into(),
            store::catalogue::FileRef {
                file: "layers/roads/hi/6-9-9.9999999999999999.pack".into(),
                ..Default::default()
            },
        );
        store::catalogue::write(&root.join("catalog"), &cat).unwrap();
        fs::create_dir_all(root.join("state/build")).unwrap();
        fs::write(root.join("state/build/manifest.json"), "{}").unwrap();
        let r = run(&store::nasfs::NasRoot::local_for_test(root), 14, false, None).unwrap();
        assert!(r.tombs == 1 && p.exists() && !tomb.exists() && r.removed == 0, "put back, decided next run: {r:?}");
        // (Its name a young file of the same bytes, uploaded again meanwhile.)
        fs::File::options().write(true).open(&p).unwrap().set_modified(SystemTime::now()).unwrap();
        fs::write(&tomb, b"x").unwrap();
        let r = run(&store::nasfs::NasRoot::local_for_test(root), 14, false, None).unwrap();
        assert!(r.tombs == 1 && !tomb.exists() && p.exists(), "its name taken: the tombstone's copy goes: {r:?}");
        // (The upload's time kept, not the tombstone's: never replaced by it.)
        assert!(
            fs::metadata(&p).unwrap().modified().unwrap().elapsed().unwrap() < Duration::from_secs(86400),
            "the new upload kept"
        );
    }

    #[test]
    fn nothing_without_a_catalogue() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("layers/roads/hi/6-1-2.0123456789abcdef.pack");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, b"x").unwrap();
        age(&p, 300);
        let r = run(&store::nasfs::NasRoot::local_for_test(d.path()), 14, false, None).unwrap();
        assert_eq!((r.removed, r.files_seen), (0, 0));
        assert!(p.exists());
    }

    /// Task #701: a removed step's names (`RETIRED`) leave the manifest in one save, the rest kept;
    /// its files then go once old, but one a catalogue of the window names stays; a run after
    /// changes nothing (no names left, the manifest not written).
    #[test]
    fn removed_steps_names_leave_the_manifest_and_their_files_go() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let (v1, v1max, v3, other) = (
            "sources/terrain-z8-v1.1111111111111111.pack",
            "sources/terrain-z8-v1-max.2222222222222222.f32",
            "sources/terrain-z8-v3.3333333333333333.pack",
            "base/6-1-1.4444444444444444.sect",
        );
        for f in [v1, v1max, v3, other, "sources/terrain-z8-README"] {
            fs::create_dir_all(root.join(f).parent().unwrap()).unwrap();
            fs::write(root.join(f), b"x").unwrap();
            age(&root.join(f), 30);
        }
        // (A tombstone of one, a GC stopped between: put back, then judged next run.)
        fs::write(root.join(format!("{v1max}{TOMB}")), b"x").unwrap();
        fs::remove_file(root.join(v1max)).unwrap();
        fs::create_dir_all(root.join("state/build")).unwrap();
        let manifest = serde_json::json!({
            "sources/terrain-z8-v1": v1,
            "sources/terrain-z8-v1-max": v1max,
            "sources/terrain-z8-v3": v3,
            "base/6-1-1": other,
        });
        let mpath = root.join("state/build/manifest.json");
        fs::write(&mpath, manifest.to_string()).unwrap();
        // The catalogue in the window still serves v3: kept.
        let mut cat = store::catalogue::Catalogue::new(1);
        cat.files.insert(
            "sources/terrain-z8-v3".into(),
            store::catalogue::FileRef { file: v3.into(), ..Default::default() },
        );
        store::catalogue::write(&root.join("catalog"), &cat).unwrap();
        let nas = store::nasfs::NasRoot::local_for_test(root);
        let r = run(&nas, 14, false, None).unwrap();
        assert_eq!(r.retired_dropped, 3, "{:?}", r.warnings);
        let m: std::collections::BTreeMap<String, String> = serde_json::from_slice(&fs::read(&mpath).unwrap()).unwrap();
        assert_eq!(m.keys().collect::<Vec<_>>(), ["base/6-1-1"]);
        assert_eq!((r.retired_removed, r.tombs), (1, 1));
        assert!(!root.join(v1).exists() && root.join(v3).exists() && root.join(other).exists());
        assert!(root.join(v1max).exists() && root.join("sources/terrain-z8-README").exists());
        // Again: nothing to drop, the manifest left as it was; the one put back goes now.
        let before = fs::metadata(&mpath).unwrap().modified().unwrap();
        age(&root.join(v1max), 30);
        let r = run(&nas, 14, false, None).unwrap();
        assert_eq!((r.retired_dropped, r.retired_removed), (0, 1));
        assert_eq!(fs::metadata(&mpath).unwrap().modified().unwrap(), before);
        assert!(!root.join(v1max).exists() && root.join(v3).exists());
        // A dry run counts, writing nothing.
        fs::write(&mpath, manifest.to_string()).unwrap();
        assert_eq!(run(&nas, 14, true, None).unwrap().retired_dropped, 3, "a dry run counts what it would drop");
        assert_eq!(fs::read(&mpath).unwrap(), manifest.to_string().as_bytes());
    }

    /// Task #52: the old pipeline's converted data (`global/legacy/`) is retired: its names leave
    /// the manifest and its files go by the folder sweep alone (once, counted as retired), its
    /// tombstones put back; nothing else under `global/` is dropped, a look-alike folder included.
    #[test]
    fn the_legacy_folder_is_retired_and_nothing_else_under_global() {
        for l in ["global/legacy/ferries", "global/legacy/details-poi", "global/legacy/dem-stats"] {
            assert!(retired(l), "{l}");
        }
        for l in ["global/legacy", "global/legacyx/ferries", "global/railfreq", "global/heritage/layer-summary"] {
            assert!(!retired(l), "{l}");
        }
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let (ferries, poi, stats, rail, near) = (
            "global/legacy/ferries.1111111111111111.json",
            "global/legacy/details-poi.2222222222222222.jsonl",
            "global/legacy/dem-stats.3333333333333333.json",
            "global/railfreq.4444444444444444.bin",
            "global/legacyx/ferries.5555555555555555.json",
        );
        for f in [ferries, poi, stats, rail, near] {
            fs::create_dir_all(root.join(f).parent().unwrap()).unwrap();
            fs::write(root.join(f), b"xy").unwrap();
            age(&root.join(f), 30);
        }
        fs::write(root.join(format!("{stats}{TOMB}")), b"xy").unwrap();
        fs::remove_file(root.join(stats)).unwrap();
        fs::create_dir_all(root.join("state/build")).unwrap();
        let manifest = serde_json::json!({
            "global/legacy/ferries": ferries,
            "global/legacy/details-poi": poi,
            "global/legacy/dem-stats": stats,
            "global/railfreq": rail,
            "global/legacyx/ferries": near,
        });
        let mpath = root.join("state/build/manifest.json");
        fs::write(&mpath, manifest.to_string()).unwrap();
        // (Uploads never verified: the retired ones leave the pending too, the rest stay.)
        let ppath = root.join("state/build/pending.json");
        fs::write(&ppath, serde_json::json!({ferries: "a", poi: "b", rail: "c"}).to_string()).unwrap();
        store::catalogue::write(&root.join("catalog"), &store::catalogue::Catalogue::new(1)).unwrap();
        let nas = store::nasfs::NasRoot::local_for_test(root);
        let r = run(&nas, 14, false, None).unwrap();
        assert_eq!(r.retired_dropped, 3, "{:?}", r.warnings);
        let p: std::collections::BTreeMap<String, String> = serde_json::from_slice(&fs::read(&ppath).unwrap()).unwrap();
        assert_eq!(p.keys().collect::<Vec<_>>(), [rail]);
        let m: std::collections::BTreeMap<String, String> = serde_json::from_slice(&fs::read(&mpath).unwrap()).unwrap();
        assert_eq!(m.keys().collect::<Vec<_>>(), ["global/legacyx/ferries", "global/railfreq"]);
        assert_eq!((r.retired_removed, r.retired_bytes, r.removed, r.tombs), (2, 4, 2, 1));
        assert!(!root.join(ferries).exists() && !root.join(poi).exists() && root.join(stats).exists());
        assert!(root.join(rail).exists() && root.join(near).exists());
        age(&root.join(stats), 30);
        let r = run(&nas, 14, false, None).unwrap();
        assert_eq!((r.retired_dropped, r.retired_removed, r.removed), (0, 1, 1));
        assert!(!root.join(stats).exists() && root.join(rail).exists() && root.join(near).exists());
        // With nothing else under global/ named, only the legacy folder is listed: an old,
        // content-named file beside it no one names stays (the folder sweep never took it).
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let (ferries, other) = ("global/legacy/ferries.1111111111111111.json", "global/railfreq.4444444444444444.bin");
        for f in [ferries, other] {
            fs::create_dir_all(root.join(f).parent().unwrap()).unwrap();
            fs::write(root.join(f), b"xy").unwrap();
            age(&root.join(f), 30);
        }
        fs::create_dir_all(root.join("state/build")).unwrap();
        fs::write(
            root.join("state/build/manifest.json"),
            serde_json::json!({"global/legacy/ferries": ferries}).to_string(),
        )
        .unwrap();
        store::catalogue::write(&root.join("catalog"), &store::catalogue::Catalogue::new(1)).unwrap();
        let r = run(&store::nasfs::NasRoot::local_for_test(root), 14, false, None).unwrap();
        assert_eq!((r.retired_dropped, r.retired_removed, r.removed), (1, 1, 1), "{:?}", r.warnings);
        assert!(!root.join(ferries).exists() && root.join(other).exists());
    }

    /// Task #701: in the pool the drop is the gc job's hand-off (merged by the lead), never a
    /// save of the records; a name outside `RETIRED` it can't remove.
    #[test]
    fn in_the_pool_the_drop_is_a_hand_off() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("nas");
        fs::create_dir_all(root.join("state/build")).unwrap();
        let manifest = r#"{"sources/terrain-z8-v3": "sources/terrain-z8-v3.3333333333333333.pack", "base/6-1-1": "base/6-1-1.4444444444444444.sect"}"#;
        fs::write(root.join("state/build/manifest.json"), manifest).unwrap();
        let nas = store::nasfs::NasRoot::local_for_test(&root);
        let hand = d.path().join("handoff");
        let mut out = crate::out::Out::open(&nas, &d.path().join("s"), STEP).unwrap().handing_off(&hand);
        out.remove("sources/terrain-z8-v3");
        out.unpend("sources/terrain-z8-v3.3333333333333333.pack");
        out.save().unwrap();
        let hs = crate::handoff::written_in(&hand).unwrap().unwrap();
        assert_eq!(hs.len(), 1);
        // (Its upload, never verified, leaves the records' pending as the lead merges it.)
        assert_eq!(hs[0].checked, ["sources/terrain-z8-v3.3333333333333333.pack".to_string()]);
        assert_eq!(
            hs[0].changes,
            [("sources/terrain-z8-v3".to_string(), None)].into_iter().collect::<std::collections::BTreeMap<_, _>>()
        );
        assert_eq!(fs::read_to_string(root.join("state/build/manifest.json")).unwrap(), manifest);
        let mut out = crate::out::Out::open(&nas, &d.path().join("s"), STEP).unwrap().handing_off(&hand);
        out.remove("base/6-1-1");
        assert!(out.save().is_err());
    }

    /// Task #679: the depth's targets as the agent's plan finds them (z3 tile 0/4, z6 tile 1/33).
    fn keep() -> DepthKeep {
        DepthKeep { at: crate::agent::jobs::now_s(), lo: [(0, 4)].into(), hi: [(1, 33)].into() }
    }

    /// Task #679: a depth pack goes only when no current target names its tile: a z6 tile's hi
    /// packs (depth, classes, shading) by it, a z3 tile's lo packs (the assembly's and the floor's
    /// shading's) by it; nothing else, whatever its tile.
    #[test]
    fn a_depth_pack_is_an_orphan_only_when_no_target_names_its_tile() {
        let k = keep();
        for l in ["layers/depth/hi/6-1-33", "layers/depth-class/hi/6-1-33", "layers/depth-shade/hi/6-1-33"] {
            assert!(!k.orphan(l), "{l}");
        }
        for l in ["layers/depth/lo/3-0-4", "layers/depth-class/lo/3-0-4", "layers/depth-shade/lo/3-0-4"] {
            assert!(!k.orphan(l), "{l}");
        }
        for l in [
            "layers/depth/hi/6-2-33",
            "layers/depth-class/hi/6-1-34",
            "layers/depth-shade/lo/3-1-4",
            "layers/depth/lo/3-0-5",
        ] {
            assert!(k.orphan(l), "{l}");
        }
        // (Not a depth pack, or not a tile as the depth names them: never.)
        for l in [
            "layers/terrain/hi/6-2-33",
            "layers/depthx/hi/6-2-33",
            "layers/depth/hi/7-2-33",
            "layers/depth/lo/6-2-33",
            "layers/depth/hi/6-2",
            "layers/depth/root",
            "sources/depth/6-2-33",
        ] {
            assert!(!k.orphan(l), "{l}");
        }
    }

    /// Task #679: the depth's targets are read only fresh; none there, none read (no depth name
    /// dropped); one stale or damaged, an error (said, no depth name dropped).
    #[test]
    fn the_depth_targets_are_read_only_fresh() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(DEPTH_KEEP_FILE);
        let now = crate::agent::jobs::now_s();
        assert!(DepthKeep::read(&p, now).unwrap().is_none());
        let k = keep();
        fs::write(&p, serde_json::to_vec(&k).unwrap()).unwrap();
        assert_eq!(DepthKeep::read(&p, now).unwrap(), Some(k.clone()));
        assert!(DepthKeep::read(&p, k.at + DEPTH_KEEP_FRESH_S + 1).is_err());
        fs::write(&p, b"{\"at\": ").unwrap();
        assert!(DepthKeep::read(&p, now).is_err());
    }

    /// Task #679: the depth packs no current target names leave the manifest in one save, with the
    /// removed steps' names; every other name stays (a target's packs, other layers' of the same
    /// tile). Their files then go once old, but one a catalogue of the window names stays; a file a
    /// running depth job uploaded (its entry unmerged) stays. Without the targets, no depth name
    /// goes; a dry run counts them, writing nothing; a run after drops nothing.
    #[test]
    fn depth_packs_no_target_names_leave_the_manifest_and_their_files_go() {
        use crate::pool::journal::{self, Entry, LeaseId};
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let kept_hi = "layers/depth/hi/6-1-33.1111111111111111.pack";
        let kept_lo = "layers/depth-shade/lo/3-0-4.2222222222222222.pack";
        let gone_hi = "layers/depth/hi/6-2-33.3333333333333333.pack";
        let gone_class = "layers/depth-class/hi/6-2-33.4444444444444444.pack";
        let gone_lo = "layers/depth/lo/3-5-5.5555555555555555.pack";
        let terrain = "layers/terrain/hi/6-2-33.6666666666666666.pack";
        let z8 = "sources/terrain-z8-v1.7777777777777777.pack";
        // (A running depth job's upload for the orphan tile, its entry written, not merged.)
        let running = "layers/depth-shade/hi/6-2-33.8888888888888888.pack";
        for f in [kept_hi, kept_lo, gone_hi, gone_class, gone_lo, terrain, z8, running] {
            fs::create_dir_all(root.join(f).parent().unwrap()).unwrap();
            fs::write(root.join(f), b"x").unwrap();
            age(&root.join(f), 30);
        }
        let logical = |c: &str| c.split_once('.').unwrap().0.to_string();
        let manifest: std::collections::BTreeMap<String, String> =
            [kept_hi, kept_lo, gone_hi, gone_class, gone_lo, terrain, z8]
                .iter()
                .map(|c| (logical(c), c.to_string()))
                .collect();
        fs::create_dir_all(root.join("state/build")).unwrap();
        let mpath = root.join("state/build/manifest.json");
        let text = serde_json::to_string(&manifest).unwrap();
        fs::write(&mpath, &text).unwrap();
        // A catalogue in the window still serves the orphan's classes: kept.
        let mut cat = store::catalogue::Catalogue::new(1);
        cat.files
            .insert(logical(gone_class), store::catalogue::FileRef { file: gone_class.into(), ..Default::default() });
        store::catalogue::write(&root.join("catalog"), &cat).unwrap();
        let nas = store::nasfs::NasRoot::local_for_test(root);
        let mut h = crate::handoff::Handoff::default();
        h.changes.insert(logical(running), Some(running.into()));
        let e = Entry {
            member: "m-0000000000000002".into(),
            lease: LeaseId { term: 1, n: 4 },
            step: "depth".into(),
            handoff: h,
            at: 1_791_500_000,
        };
        journal::write(&nas, &e).unwrap();
        let k = keep();
        // Without the targets: no depth name goes (the removed step's does).
        assert_eq!(drop_names(&nas, &d.path().join("s"), None, true).unwrap(), (1, 0));
        // A dry run counts, writing nothing.
        let r = run(&nas, 14, true, Some(&k)).unwrap();
        assert_eq!((r.retired_dropped, r.depth_dropped), (1, 3), "{:?}", r.warnings);
        assert_eq!(fs::read_to_string(&mpath).unwrap(), text);
        let r = run(&nas, 14, false, Some(&k)).unwrap();
        assert_eq!((r.retired_dropped, r.depth_dropped), (1, 3), "{:?}", r.warnings);
        let m: std::collections::BTreeMap<String, String> = serde_json::from_slice(&fs::read(&mpath).unwrap()).unwrap();
        assert_eq!(
            m.keys().collect::<Vec<_>>(),
            ["layers/depth-shade/lo/3-0-4", "layers/depth/hi/6-1-33", "layers/terrain/hi/6-2-33"]
        );
        for f in [kept_hi, kept_lo, terrain, gone_class, running] {
            assert!(root.join(f).exists(), "{f}");
        }
        for f in [gone_hi, gone_lo, z8] {
            assert!(!root.join(f).exists(), "{f}");
        }
        // Again: nothing to drop, the manifest left as it was.
        let before = fs::metadata(&mpath).unwrap().modified().unwrap();
        let r = run(&nas, 14, false, Some(&k)).unwrap();
        assert_eq!((r.retired_dropped, r.depth_dropped), (0, 0));
        assert_eq!(fs::metadata(&mpath).unwrap().modified().unwrap(), before);
    }

    /// Task #679: a depth job leased for a tile the coverage has left since keeps its packs; other
    /// steps' leases keep nothing.
    #[test]
    fn a_running_depth_job_keeps_its_tiles_packs() {
        let leased = [
            ("depth".to_string(), "6/2/33".to_string()),
            ("depth-shade".to_string(), "3/1/4".to_string()),
            ("depth-lo".to_string(), "3/5/5".to_string()),
            ("terrain-hi".to_string(), "6/9/9".to_string()),
            ("depth".to_string(), "3/7/7".to_string()),
        ];
        let k = keep().with_leased(&leased);
        for l in [
            "layers/depth/hi/6-2-33",
            "layers/depth-shade/lo/3-1-4",
            "layers/depth-class/lo/3-5-5",
            "layers/depth/hi/6-1-33",
        ] {
            assert!(!k.orphan(l), "{l}");
        }
        for l in ["layers/depth/hi/6-9-9", "layers/depth/lo/3-7-7"] {
            assert!(k.orphan(l), "{l}");
        }
    }
}
