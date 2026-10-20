//! Removing replaced files from the NAS (docs/plan.md §3, GC): every content-named file that
//! neither the newest catalog, nor a catalog of the last `keep_days`, nor the build's manifest
//! nor (with the pool) its newest records or a journal entry they don't reflect yet (`unmerged`)
//! references, and that is itself older than `keep_days`; abandoned `.tmp` files; old catalogs.
//! So it runs beside other jobs: a job's files are young, or named by its entry once it's written.
//!
//! Deletions go through SMB: on this share they're permanent (no Recycle Bin entry; tested
//! 2026-10-02), and the M4 can't use SSH unattended (1Password asks each session).
//!
//! Only the folders catalogs index are swept (`base/`, `global/`, `layers/`, `hidata/`, …: the
//! first path component of every referenced file), and the sources of passes older than the newest
//! complete one (`sources/osm/<date>/`, `sources/items/<date>/`): their content-named files by the
//! same rule, the rest (the planet download) once the newer pass has been complete for `keep_days`.
//! The gate's checked copies (`sources/inputs/`, docs/inputs.md §4.9) by the same rule, kept while
//! the manifest (or the newest records) names them, an accepted index it names lists them, a
//! journal entry not yet merged does, or an index made or replaced within `keep_days` lists them
//! (`inputs_kept`; none swept when one of those can't be read).
//! The newest pass, a planet waiting for its pass, the rest of `sources/`, the user's folders (the
//! drop boxes), state and the app are never touched. With no readable catalog nothing is deleted.

use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub catalogs_kept: usize,
    pub catalogs_removed: usize,
    pub referenced: usize,
    pub files_seen: usize,
    pub removed: usize,
    pub removed_bytes: u64,
    pub tmp_removed: usize,
    /// Unreferenced but too young to remove.
    pub young: usize,
    /// Of the removed, files of retired passes' sources (and their bytes).
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
    /// The files put back this run: left alone by its sweeps (the next run decides).
    #[serde(skip)]
    pub restored: BTreeSet<PathBuf>,
    /// What it passed over and why (the agent's status says them as the job's note).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    pub dry_run: bool,
}

/// Folders never swept, whatever a catalog says (`sources/` but its retired passes and the gate's
/// checked copies, `sources/inputs/`, swept on their own rules).
const NEVER: [&str; 7] = ["translations", "descriptions", "inputs", "state", "app", "nas", "sources"];

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
/// as any file no catalog, manifest or records name.
pub const PASS_OVER_S: u64 = 3 * 86400;
const _: () = assert!(PASS_OVER_S >= crate::pool::driver::UNREADABLE_S + 2 * 86400);

/// `now`: unix seconds, which `PASS_OVER_S` is counted to (by the entry's day, its key's: one
/// whose day is before `now - PASS_OVER_S`'s was written longer ago than that).
pub fn unmerged(root: &Path, now: u64) -> Result<Unmerged> {
    use crate::pool::{journal, records::Records, term};
    let mut u = Unmerged::default();
    match std::fs::metadata(root.join(journal::DIR)) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(u),
        Err(e) => return Err(e).context("the journal"),
    }
    let nas = crate::pool::nas::Share::new(root);
    let cur = term::current(&nas).context("the current term")?;
    let r = Records::newest(&nas, cur.term, true).context("the newest records")?.unwrap_or_default();
    // (Led: a term readable whole; only then does anyone refuse.)
    let led = cur.lead.is_some();
    let cutoff = journal::day(now.saturating_sub(PASS_OVER_S)).unwrap_or_default();
    u.files.extend(r.manifest.values().cloned());
    u.records = r.manifest.iter().map(|(l, c)| (l.clone(), c.clone())).collect();
    u.files.extend(r.pending.keys().cloned());
    // (The days before the records' horizon are forgotten, every entry of them merged: not listed.)
    let since = (!r.horizon.is_empty()).then_some(r.horizon.as_str());
    for key in journal::list(&nas, since).context("the journal")? {
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
    root: &Path,
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
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("list {}", dir.display())),
        };
        for e in rd {
            let e = e.with_context(|| format!("list {}", dir.display()))?;
            meta(1, 0, 0);
            if e.file_type()?.is_dir() {
                stack.push(e.path());
                continue;
            }
            let rel = e.path().strip_prefix(root).unwrap_or(&e.path()).to_string_lossy().replace('\\', "/");
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
    if crate::control::draining() {
        note_cost();
        crate::control::stop_paused(STEP);
    }
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
pub fn run(root: &Path, keep_days: u64, dry_run: bool) -> Result<Report> {
    STARTED.get_or_init(std::time::Instant::now);
    if std::env::var_os("SCENIC_COSTS").is_some() {
        COUNTING.set(true);
        crate::costin::begin();
    }
    let r = run_once(root, keep_days, dry_run);
    note_cost();
    r
}

fn run_once(root: &Path, keep_days: u64, dry_run: bool) -> Result<Report> {
    use crate::timings::{phase, Class};
    let keep = Duration::from_secs(keep_days * 86400);
    let now = SystemTime::now();
    let old = |t: SystemTime| now.duration_since(t).is_ok_and(|d| d > keep);
    let mut rep = Report { dry_run, ..Default::default() };

    // Catalogs: the newest always, and every one of the last `keep_days`.
    let cat_dir = root.join("catalog");
    let read = phase("catalogs read", Class::NasRead);
    let ns = if cat_dir.exists() { store::catalog::list(&cat_dir).context("list catalogs")? } else { Vec::new() };
    // Before the first catalog there's nothing to keep track of, so nothing is removed.
    let Some(&latest) = ns.iter().max() else { return Ok(rep) };
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    let mut drop_cats: Vec<PathBuf> = Vec::new();
    // (How far it is, for the status: the catalogs read, then the folders swept.)
    crate::agent::jobs::stage(0, 2, "steps (reading the catalogs)");
    // (Each catalog's file, whole or a delta: store::catalog::path_n.)
    let mut in_window: BTreeSet<u64> = BTreeSet::new();
    for &n in &ns {
        let p = store::catalog::path_n(&cat_dir, n).with_context(|| format!("catalog {n} gone meanwhile"))?;
        meta(0, 1, 0);
        let mtime =
            std::fs::metadata(&p).and_then(|m| m.modified()).with_context(|| format!("stat {}", p.display()))?;
        if n == latest || !old(mtime) {
            in_window.insert(n);
        }
    }
    // A delta kept keeps the catalogs it's read through, back to a whole one (their files aren't
    // kept for them: only the catalogs in the window are what's served). One that can't be read
    // stops the sweep.
    let mut kept_cats = in_window.clone();
    for &n in &in_window {
        kept_cats.extend(
            store::catalog::chain(&cat_dir, n).with_context(|| format!("the catalogs catalog {n} is read through"))?,
        );
    }
    for &n in ns.iter().filter(|n| !kept_cats.contains(n)) {
        drop_cats.extend(store::catalog::path_n(&cat_dir, n));
    }
    // Read oldest first, each delta applied to the one before when that's just been read (not
    // its chain read again).
    let mut prev: Option<store::catalog::Catalog> = None;
    for (k, &n) in in_window.iter().enumerate() {
        crate::agent::jobs::within(k as f64 / in_window.len() as f64);
        let p = store::catalog::path_n(&cat_dir, n).with_context(|| format!("catalog {n} gone meanwhile"))?;
        // A catalog in the window that can't be read stops the sweep: its files might be in use.
        let delta = (p.file_name() == Some(std::ffi::OsStr::new(&store::catalog::delta_file_name(n))))
            .then(|| std::fs::read(&p).ok().and_then(|b| store::catalog::Delta::decode(&b).ok()))
            .flatten();
        let cat = match (delta, &prev) {
            (Some(d), Some(c)) if c.n == d.of => d.apply(c).with_context(|| format!("read {}", p.display()))?,
            _ => store::catalog::read_n(&cat_dir, n).with_context(|| format!("read {}", p.display()))?,
        };
        read.count(std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0), 1);
        referenced.extend(cat.files.values().map(|f| f.file.clone()));
        rep.catalogs_kept += 1;
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

    crate::agent::jobs::stage(1, 2, "steps (sweeping the folders)");
    // (Listing the folders and removing what's due, file by file: one phase, the removals counted.)
    let sweep = phase("folders swept", Class::NasWrite);
    for (k, top) in tops.iter().enumerate() {
        yield_point();
        crate::agent::jobs::within(k as f64 / tops.len() as f64);
        let mut stack = vec![root.join(top)];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
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
                let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().into_owned();
                let name = e.file_name().to_string_lossy().into_owned();
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
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
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
                let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().into_owned();
                let mtime = md.modified().unwrap_or(now);
                if e.file_name().to_string_lossy().ends_with(TOMB) {
                    put_back(&p, dry_run, &mut rep);
                    continue;
                }
                if e.file_name().to_string_lossy().ends_with(".tmp") {
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
        let done_at = std::fs::read_dir(root.join("sources/osm").join(&latest))
            .ok()
            .and_then(|rd| {
                rd.flatten()
                    .find(|e| e.file_name().to_str().is_some_and(|n| n.starts_with("pass.") && n.ends_with(".json")))
            })
            .and_then(|e| e.metadata().ok()?.modified().ok());
        let plain_due = done_at.is_some_and(old);
        for kind in ["osm", "items"] {
            let Ok(rd) = std::fs::read_dir(root.join("sources").join(kind)) else { continue };
            let mut dates: Vec<String> = rd
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|d| crate::osmpass::is_date(d) && *d < latest)
                .collect();
            dates.sort();
            for d in dates {
                yield_point();
                let dir = root.join("sources").join(kind).join(&d);
                sweep_retired(root, &dir, &referenced, &old, plain_due, dry_run, &mut rep)?;
            }
        }
    }
    let _p = phase("old catalogs removed", Class::NasWrite);
    for p in drop_cats {
        remove(&p, dry_run)?;
        rep.catalogs_removed += 1;
    }
    Ok(rep)
}

/// One retired pass's folder: content-named files no catalog or manifest names, once old; any other
/// file once `plain_due`; then the folders left empty.
fn sweep_retired(
    root: &Path,
    dir: &Path,
    referenced: &BTreeSet<String>,
    old: &dyn Fn(SystemTime) -> bool,
    plain_due: bool,
    dry_run: bool,
    rep: &mut Report,
) -> Result<()> {
    let mut dirs = vec![dir.to_path_buf()];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            meta(1, 1, 0);
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                stack.push(p.clone());
                dirs.push(p);
                continue;
            }
            rep.files_seen += 1;
            if e.file_name().to_string_lossy().ends_with(TOMB) {
                put_back(&p, dry_run, rep);
                continue;
            }
            if rep.restored.contains(&p) {
                continue;
            }
            let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().into_owned();
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
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for d in dirs {
        meta(1, 0, 0);
        if std::fs::read_dir(&d).is_ok_and(|mut rd| rd.next().is_none()) && !dry_run {
            meta(0, 0, 1);
            std::fs::remove_dir(&d).ok();
        }
    }
    Ok(())
}

/// Whether file `p` is still old by a fresh look at its time (not the listing's: a job reusing a
/// content-named file touches it, and GC's listing may be minutes old by the time it gets there).
fn still_old(p: &Path, old: &dyn Fn(SystemTime) -> bool) -> bool {
    meta(0, 1, 0);
    std::fs::metadata(p).ok().and_then(|m| m.modified().ok()).is_some_and(old)
}

/// What a file GC is about to remove is renamed to first (`remove_unless_touched`).
const TOMB: &str = store::naming::TOMB;

/// A tombstone a sweep finds (left by a GC stopped between its rename and its removal, a put-back
/// that failed, or another GC's still in flight): never removed here, but put back under its name
/// when that's free, for the usual rules to decide next run; when its name has a file again (a job
/// uploaded the same bytes meanwhile), the tombstone's copy is the same bytes and goes. Another
/// GC's in flight finds its tombstone gone and keeps the file (`remove_unless_touched`).
fn put_back(tomb: &Path, dry_run: bool, rep: &mut Report) {
    let Some(name) = tomb.to_str().and_then(|t| t.strip_suffix(TOMB)).map(PathBuf::from) else { return };
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
fn restore(tomb: &Path, name: &Path) -> std::io::Result<()> {
    meta(0, 0, 1);
    match store::naming::rename_no_replace(tomb, name) {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => std::fs::remove_file(tomb),
        r => r,
    }
}

/// Removes content-named file `p`, unless a job reused it by name meanwhile: renamed to a tombstone
/// first (a job reusing it now finds no file of its name, and uploads it again: out::Out::put_file),
/// its time then looked at again, the tombstone's: touched since GC listed it (a job had it open
/// before the rename), put back; else removed. True when it went.
fn remove_unless_touched(p: &Path, old: &dyn Fn(SystemTime) -> bool, dry_run: bool) -> Result<bool> {
    if dry_run {
        eprintln!("gc: would remove {}", p.display());
        return Ok(still_old(p, old));
    }
    let mut tomb = p.as_os_str().to_owned();
    tomb.push(TOMB);
    let tomb = PathBuf::from(tomb);
    // (Not renamed aside, for whatever reason: kept this run, as a young file is.)
    // (A rename and a removal: each a metadata write, counted as a delete.)
    meta(0, 0, 1);
    if let Err(e) = std::fs::rename(p, &tomb) {
        if e.kind() != std::io::ErrorKind::NotFound {
            eprintln!("gc: {} not set aside ({e}): kept this run", p.display());
        }
        return Ok(false);
    }
    if still_old(&tomb, old) {
        meta(0, 0, 1);
        return match std::fs::remove_file(&tomb) {
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

fn remove(p: &Path, dry_run: bool) -> Result<()> {
    if dry_run {
        eprintln!("gc: would remove {}", p.display());
        return Ok(());
    }
    meta(0, 0, 1);
    std::fs::remove_file(p).with_context(|| format!("remove {}", p.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, FileTimes};

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
        let mut cat = store::catalog::Catalog::new(1);
        cat.files.insert(
            "layers/roads/hi/6-1-2".into(),
            store::catalog::FileRef { file: kept.into(), ..Default::default() },
        );
        store::catalog::write(&root.join("catalog"), &cat).unwrap();
        let r = run(root, 14, false).unwrap();
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
        let mut cat = store::catalog::Catalog::new(1);
        cat.files.insert(
            "layers/a/6-1-2".into(),
            store::catalog::FileRef { file: "layers/a/6-1-2.0123456789abcdef.pack".into(), ..Default::default() },
        );
        store::catalog::write(&root.join("catalog"), &cat).unwrap();
        crate::costin::begin();
        COUNTING.set(true);
        let r = run_once(root, 14, false).unwrap();
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

    #[test]
    fn keeps_the_newest_catalog_however_old() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let mk = |n: u64, file: &str| {
            let p = root.join(file);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, b"x").unwrap();
            age(&p, 60);
            let mut cat = store::catalog::Catalog::new(n);
            let logical = file.rsplit_once('.').unwrap().0.rsplit_once('.').unwrap().0;
            cat.files.insert(logical.into(), store::catalog::FileRef { file: file.into(), ..Default::default() });
            let c = store::catalog::write(&root.join("catalog"), &cat).unwrap();
            age(&c, 60);
        };
        mk(1, "layers/a/hi/6-1-1.1111111111111111.pack");
        mk(2, "layers/a/hi/6-1-1.2222222222222222.pack");
        mk(3, "layers/a/hi/6-1-1.3333333333333333.pack");
        let r = run(root, 14, false).unwrap();
        assert_eq!((r.catalogs_kept, r.catalogs_removed, r.removed), (1, 2, 2));
        assert!(root.join("layers/a/hi/6-1-1.3333333333333333.pack").exists());
        assert_eq!(store::catalog::latest(&root.join("catalog")).unwrap().unwrap().n, 3);
        assert_eq!(store::catalog::next_n(&root.join("catalog")).unwrap(), 4);
    }

    #[test]
    fn a_delta_kept_keeps_the_catalogs_its_read_through_and_its_files() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let dir = root.join("catalog");
        let mut prev: Option<store::catalog::Catalog> = None;
        // 1 whole and old, 2 a delta and old, 3 and 4 deltas in the window (3 over 2, 4 over 3).
        for (n, old) in [(1, true), (2, true), (3, false), (4, false)] {
            let file = format!("layers/a/hi/6-1-{n}.{}.pack", format!("{n}").repeat(16));
            let p = root.join(&file);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, b"x").unwrap();
            age(&p, 60);
            let mut cat = prev.clone().unwrap_or_else(|| store::catalog::Catalog::new(1));
            cat.n = n;
            cat.files.insert(format!("layers/a/hi/6-1-{n}"), store::catalog::FileRef { file, ..Default::default() });
            let c = match &prev {
                None => store::catalog::write(&dir, &cat).unwrap(),
                Some(b) => store::catalog::write_delta(&dir, b, &cat).unwrap(),
            };
            if old {
                age(&c, 60);
            }
            prev = Some(cat);
        }
        let r = run(root, 14, false).unwrap();
        // (3 and 4 kept, read; 1 and 2 kept as what 3 is read through, their files not kept for them
        // but 3's names them all.)
        assert_eq!((r.catalogs_kept, r.catalogs_removed, r.removed), (2, 0, 0));
        assert_eq!(store::catalog::latest(&dir).unwrap().unwrap(), prev.unwrap());
        assert_eq!(store::catalog::list(&dir).unwrap(), [4, 3, 2, 1]);
    }

    #[test]
    fn the_build_manifest_keeps_its_files() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let f = "base/6-1-1.4444444444444444.sect";
        fs::create_dir_all(root.join("base")).unwrap();
        fs::write(root.join(f), b"x").unwrap();
        age(&root.join(f), 60);
        let cat = store::catalog::Catalog::new(1);
        store::catalog::write(&root.join("catalog"), &cat).unwrap();
        // Not in the catalog, old, but the build uploaded it: kept. (A catalog file elsewhere so
        // `base/` is swept at all.)
        let mut cat2 = store::catalog::Catalog::new(2);
        cat2.files.insert(
            "base/6-1-2".into(),
            store::catalog::FileRef { file: "base/6-1-2.5555555555555555.sect".into(), ..Default::default() },
        );
        store::catalog::write(&root.join("catalog"), &cat2).unwrap();
        fs::create_dir_all(root.join("state/build")).unwrap();
        fs::write(root.join("state/build/manifest.json"), format!("{{\"base/6-1-1\": \"{f}\"}}")).unwrap();
        let r = run(root, 14, false).unwrap();
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
        store::catalog::write(&root.join("catalog"), &store::catalog::Catalog::new(1)).unwrap();
        let r = run(root, 14, false).unwrap();
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
        let r = run(root, 14, false).unwrap();
        assert_eq!(r.retired_removed, 1);
        assert!(root.join("sources/osm/2026-03-01/planet.osm.pbf").exists());
    }

    /// The gate's copies (docs/inputs.md §4.9): kept while the manifest's index or report names
    /// them, or a journal entry not merged yet does; the rest of `sources/` and the drop boxes
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
        put("inputs/_gate-test/a.jsonl", "x", 300); // the drop box: never
        put("sources/registers/legacy.9999999999999999.tar.zst", "x", 300); // the rest of sources/: never
        store::catalog::write(&root.join("catalog"), &store::catalog::Catalog::new(1)).unwrap();
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
        crate::pool::journal::write(&crate::pool::nas::Share::new(root), &entry).unwrap();
        let r = run(root, 14, false).unwrap();
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
        let r = run(root, 14, false).unwrap();
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
        let r = run(root, 14, false).unwrap();
        assert_eq!(r.inputs_removed, 0, "{r:?}");
        assert!(
            root.join(format!("{u}/a.cccccccccccccccc.jsonl")).exists()
                && root.join(format!("{n}/c.aaaaaaaaaaaaaaae.zip")).exists()
        );
        // 15 days after it was replaced (the state that named it that old too): gone, with its index.
        age(&root.join(format!("{u}/@listed.eeeeeeeeeeeeeeee.json")), 15);
        let r = run(root, 14, false).unwrap();
        assert_eq!(r.inputs_removed, 3, "the copy, its index, the old state: {r:?}");
        assert!(!root.join(format!("{u}/a.cccccccccccccccc.jsonl")).exists());
        // An index named that can't be read: nothing there swept.
        put(&format!("{u}/a.aaaaaaaaaaaaaaaa.jsonl"), "x", 60);
        fs::write(root.join(format!("{u}/@index.5555555555555555.json")), b"{not json").unwrap();
        let r = run(root, 14, false).unwrap();
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
        let mut cat = store::catalog::Catalog::new(1);
        cat.files.insert(
            "layers/roads/hi/6-9-9".into(),
            store::catalog::FileRef {
                file: "layers/roads/hi/6-9-9.9999999999999999.pack".into(),
                ..Default::default()
            },
        );
        store::catalog::write(&root.join("catalog"), &cat).unwrap();
        fs::create_dir_all(root.join("state/build")).unwrap();
        fs::write(root.join("state/build/manifest.json"), "{}").unwrap();
        // An entry of a member away three weeks, written, never merged: its change and its upload.
        let nas = crate::pool::nas::Share::new(root);
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
        let r = run(root, 14, false).unwrap();
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
        let r = run(root, 14, false).unwrap();
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
            let e = format!("{:#}", run(root, 14, false).unwrap_err());
            assert!(e.contains("nothing removed") && e.contains("1-5"), "{e}");
            assert!(root.join(c).exists());
        }
        let damaged_key = format!("{}/1-6", journal::day(now).unwrap());
        entry_at(&damaged_key, b"{\"not\": \"an entry\"}");
        rec.rejected.insert(short_key, "never whole".into());
        rec.save(&nas).unwrap();
        let e = format!("{:#}", run(root, 14, false).unwrap_err());
        assert!(e.contains("damaged") && root.join(c).exists(), "{e}");
        // Both refused by the lead: passed over, the sweep goes on.
        rec.rejected.insert(damaged_key, "damaged".into());
        rec.save(&nas).unwrap();
        let r = run(root, 14, false).unwrap();
        assert!(r.removed == 1 && r.warnings.is_empty(), "{r:?}");
        // No term led (its term not readable whole): no one refuses. An entry unreadable from today
        // still fails GC; one whose day is over `PASS_OVER_S` past (ten days here, the days far
        // from the bound) is passed over, said as a warning, the sweep going on.
        fs::write(root.join(crate::pool::term::DIR).join("1.json"), b"{\"term\": ").unwrap();
        put(c, 60);
        let young = entry_at(&format!("{}/1-8", journal::day(now).unwrap()), b"{\"member\": ");
        let e = format!("{:#}", run(root, 14, false).unwrap_err());
        assert!(e.contains("1-8") && root.join(c).exists(), "{e}");
        fs::remove_file(&young).unwrap();
        entry_at(&format!("{}/1-7", journal::day(now - 10 * 86400).unwrap()), b"{\"member\": ");
        let r = run(root, 14, false).unwrap();
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
        assert!(!remove_unless_touched(&p, &old, false).unwrap());
        assert!(
            p.exists() && fs::read_dir(p.parent().unwrap()).unwrap().count() == 1,
            "back under its name, no tombstone left"
        );
        // Not touched: gone, no tombstone left.
        age(&p, 30);
        assert!(remove_unless_touched(&p, &old, false).unwrap());
        assert!(!p.exists() && fs::read_dir(p.parent().unwrap()).unwrap().count() == 0);
        // Gone already (another GC, or never there): nothing.
        assert!(!remove_unless_touched(&p, &old, false).unwrap());
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
        let mut cat = store::catalog::Catalog::new(1);
        cat.files.insert(
            "layers/roads/hi/6-9-9".into(),
            store::catalog::FileRef {
                file: "layers/roads/hi/6-9-9.9999999999999999.pack".into(),
                ..Default::default()
            },
        );
        store::catalog::write(&root.join("catalog"), &cat).unwrap();
        fs::create_dir_all(root.join("state/build")).unwrap();
        fs::write(root.join("state/build/manifest.json"), "{}").unwrap();
        let r = run(root, 14, false).unwrap();
        assert!(r.tombs == 1 && p.exists() && !tomb.exists() && r.removed == 0, "put back, decided next run: {r:?}");
        // (Its name a young file of the same bytes, uploaded again meanwhile.)
        fs::File::options().write(true).open(&p).unwrap().set_modified(SystemTime::now()).unwrap();
        fs::write(&tomb, b"x").unwrap();
        let r = run(root, 14, false).unwrap();
        assert!(r.tombs == 1 && !tomb.exists() && p.exists(), "its name taken: the tombstone's copy goes: {r:?}");
        // (The upload's time kept, not the tombstone's: never replaced by it.)
        assert!(
            fs::metadata(&p).unwrap().modified().unwrap().elapsed().unwrap() < Duration::from_secs(86400),
            "the new upload kept"
        );
    }

    #[test]
    fn nothing_without_a_catalog() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("layers/roads/hi/6-1-2.0123456789abcdef.pack");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, b"x").unwrap();
        age(&p, 300);
        let r = run(d.path(), 14, false).unwrap();
        assert_eq!((r.removed, r.files_seen), (0, 0));
        assert!(p.exists());
    }
}
