//! The check job (docs/inputs.md §4.3): one gate unit's drop box against its accepted version.
//!
//! 1. The files whose size or time differ from those the last check listed (`super::Listed`) are
//!    read and hashed (all of them with `full`); one whose bytes are those accepted is unchanged,
//!    whatever its time. The candidate is the listing the lead's key was made from (`candidate`).
//! 2. The changed files' shape checks, a removal's, then the unit's own checks over the candidate.
//! 3. Each change's verdict: clean, or held by an unaccepted finding about it; partners held
//!    together, and a file naming a held one held with it.
//! 4. The next version: the accepted one with every clean change applied.
//! 5. That version checked whole: a finding it raises that's neither accepted nor raised by the
//!    accepted version as it stands holds every change of the unit together.
//! 6. The taken files stored content-named under `sources/inputs/<unit>/`, the index and the report
//!    too, and handed off as records changes: `sources/inputs/<unit>/@index` (only when it differs),
//!    `sources/inputs/<unit>/@listed` (likewise, with the index replaced) and
//!    `sources/inputs/<unit>/@held` (the report, or none when nothing is held).
//!
//! `decide` is all of it but the reading and the writing, a function of (candidate, accepted
//! version, acceptances): the tests' subject. `run` is the job.

use super::{Checks, FileEntry, Finding, Index, Level, Listing, Report, Version};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// What a check is given.
pub struct Given<'a> {
    pub checks: &'a dyn Checks,
    /// The accepted version (None: none yet).
    pub prev: Option<&'a Index>,
    /// The sizes and times the last check listed (`super::Listed`): a file listed as it was there
    /// isn't read.
    pub prev_listed: &'a BTreeMap<String, (u64, u64)>,
    /// The candidate: the drop box as listed, its files held still.
    pub listing: &'a Listing,
    /// Files there but not held still now (changed since the listing, or too recent): as accepted
    /// if they are, absent while new; neither read nor taken as removed.
    pub unsettled: &'a BTreeSet<String>,
    pub accepted: &'a BTreeSet<String>,
    /// Every file read and hashed, whatever its size and time.
    pub full: bool,
    /// A drop-box file's bytes now.
    pub read_new: &'a dyn Fn(&str) -> Result<Vec<u8>>,
    /// An accepted file's bytes (its checked copy), by its entry.
    pub read_old: &'a dyn Fn(&FileEntry) -> Result<Vec<u8>>,
}

/// What a check decides.
#[derive(Debug)]
pub struct Decision {
    /// The next accepted version (the previous one, unchanged, when nothing was taken in).
    pub index: Index,
    /// Whether it differs from the previous one (else nothing is written for it).
    pub changed: bool,
    /// The sizes and times to compare the next listing with, and whether they differ from those
    /// given (else nothing is written for them).
    pub listed: BTreeMap<String, (u64, u64)>,
    pub listed_changed: bool,
    /// The report, when something is held.
    pub report: Option<Report>,
    /// The bytes of the files taken in, by their copies' content names, to store.
    pub store: BTreeMap<String, Arc<Vec<u8>>>,
    /// The drop-box files read (the rest were taken as unchanged by their size and time).
    pub read: Vec<String>,
}

/// A change of the candidate's.
enum Change {
    /// New or edited: its bytes and what its shape check said.
    Put { bytes: Arc<Vec<u8>>, hash: String, check: super::FileCheck },
    Removed,
}

/// The decision for unit `g.checks`'s candidate (§4.3, steps 1–5).
pub fn decide(g: &Given) -> Result<Decision> {
    let unit = g.checks.unit();
    let empty = Index { fmt: 1, unit: unit.into(), checks: format!("{unit} {}", g.checks.version()), ..Default::default() };
    let prev = g.prev.unwrap_or(&empty);
    let mut read = Vec::new();
    let mut changes: BTreeMap<String, Change> = BTreeMap::new();
    let mut findings: Vec<Finding> = Vec::new();
    // 1. What changed, by content.
    for (p, &listed) in &g.listing.files {
        if g.unsettled.contains(p) {
            continue;
        }
        let was = prev.files.get(p);
        if !g.full && was.is_some() && g.prev_listed.get(p) == Some(&listed) {
            continue;
        }
        let bytes = (g.read_new)(p).with_context(|| format!("read {p} in the drop box"))?;
        read.push(p.clone());
        let hash = store::naming::hash16(&bytes);
        if was.and_then(|w| super::hash_of(&w.file)) == Some(hash.as_str()) {
            continue;
        }
        // 2. Its shape.
        let mut check = g.checks.file(p, &bytes);
        if super::copy_name(unit, p, &hash).is_none() {
            check.findings.push(Finding::new("name", Level::Error, &[p], &[], vec![p.clone()], format!("{p}: a name the store can't keep (no backslashes)")));
        }
        findings.extend(check.findings.iter().cloned());
        changes.insert(p.clone(), Change::Put { bytes: Arc::new(bytes), hash, check });
    }
    for (p, was) in &prev.files {
        if !g.listing.files.contains_key(p) && !g.unsettled.contains(p) {
            findings.extend(g.checks.removed(p, was));
            changes.insert(p.clone(), Change::Removed);
        }
    }
    // (What isn't the drop box's shape: a folder in a unit without them.)
    for s in &g.listing.strays {
        findings.push(Finding::new("stray", Level::Error, &[s], &[], vec![s.clone()], format!("{s}: {unit} takes no folders (but todo/ and how/): move what's in it up, or out")));
    }
    // The candidate, checked whole: the files held still, and those that aren't as accepted.
    let candidate: BTreeMap<String, Option<Arc<Vec<u8>>>> = g
        .listing
        .files
        .keys()
        .filter(|p| !g.unsettled.contains(*p) || prev.files.contains_key(*p))
        .chain(prev.files.keys().filter(|p| g.unsettled.contains(*p)))
        .map(|p| {
            let b = match changes.get(p) {
                Some(Change::Put { bytes, .. }) => Some(bytes.clone()),
                _ => None,
            };
            (p.clone(), b)
        })
        .collect();
    if !changes.is_empty() || !g.listing.strays.is_empty() {
        let rd = reader(&candidate, prev, g.read_old);
        findings.extend(g.checks.whole(&Version { paths: candidate.keys().cloned().collect(), read: &rd })?);
    }
    // 3. The verdicts.
    let all: BTreeSet<String> = g.listing.files.keys().chain(prev.files.keys()).cloned().collect();
    let mut held: BTreeSet<String> = BTreeSet::new();
    for f in findings.iter().filter(|f| f.holds(g.accepted)) {
        held.extend(f.files.iter().filter(|p| changes.contains_key(*p)).cloned());
    }
    loop {
        let mut more: BTreeSet<String> = BTreeSet::new();
        for p in &held {
            more.extend(g.checks.partners(p, &all).into_iter().filter(|q| changes.contains_key(q) && !held.contains(q)));
        }
        for (p, c) in &changes {
            if let Change::Put { check, .. } = c {
                if !held.contains(p) && check.refers.iter().any(|r| held.contains(r)) {
                    more.insert(p.clone());
                }
            }
        }
        if more.is_empty() {
            break;
        }
        held.extend(more);
    }
    // 4. The next version: the clean changes applied.
    let apply = |held: &BTreeSet<String>| {
        let mut files = prev.files.clone();
        let mut taken: BTreeMap<String, Arc<Vec<u8>>> = BTreeMap::new();
        for (p, c) in &changes {
            if held.contains(p) {
                continue;
            }
            match c {
                Change::Put { bytes, hash, check } => {
                    let file = super::copy_name(unit, p, hash).expect("checked above");
                    let keyed = check.keyed.clone().unwrap_or_else(|| hash.clone());
                    files.insert(p.clone(), FileEntry { file: file.clone(), size: bytes.len() as u64, keyed, facts: check.facts.clone() });
                    taken.insert(file, bytes.clone());
                }
                Change::Removed => {
                    files.remove(p);
                }
            }
        }
        (files, taken)
    };
    let (mut files, mut taken) = apply(&held);
    let mut together = None;
    let mut whole_found: Vec<Finding> = Vec::new();
    // 5. The next version checked whole.
    if files != prev.files {
        let next: BTreeMap<String, Option<Arc<Vec<u8>>>> = files.keys().map(|p| (p.clone(), candidate.get(p).cloned().flatten().filter(|_| !held.contains(p)))).collect();
        let rd = reader(&next, prev, g.read_old);
        let found = g.checks.whole(&Version { paths: next.keys().cloned().collect(), read: &rd })?;
        let mut bad: Vec<Finding> = found.iter().filter(|f| f.holds(g.accepted)).cloned().collect();
        if !bad.is_empty() {
            // (Not one the version in use raises as it stands: that holds no change.)
            let before: BTreeMap<String, Option<Arc<Vec<u8>>>> = prev.files.keys().map(|p| (p.clone(), None)).collect();
            let rd = reader(&before, prev, g.read_old);
            let had: BTreeSet<String> = if prev.files.is_empty() { BTreeSet::new() } else { g.checks.whole(&Version { paths: before.keys().cloned().collect(), read: &rd })?.into_iter().map(|f| f.id).collect() };
            bad.retain(|f| !had.contains(&f.id));
        }
        whole_found = found;
        if !bad.is_empty() {
            together = Some(format!("the changes taken alone would make a version that raises {}: every change held together", bad.iter().map(|f| format!("\"{}\"", f.message)).collect::<Vec<_>>().join(", ")));
            held.extend(changes.keys().cloned());
            (files, taken) = apply(&held);
            findings.extend(bad);
        }
    }
    // The warnings the version is taken with: those still about files of it unchanged, and those
    // that let a change in, about files still in it (one about none of its files no longer counts:
    // its acceptance is stale).
    let lets: BTreeSet<&String> = changes.keys().filter(|p| !held.contains(*p)).collect();
    let mut accepted: BTreeMap<String, Vec<String>> = prev.accepted.iter().filter(|(_, fs)| fs.iter().all(|p| files.contains_key(p) && !lets.contains(p))).map(|(k, v)| (k.clone(), v.clone())).collect();
    if files != prev.files {
        for f in findings.iter().chain(&whole_found) {
            let lets_in = f.files.iter().any(|p| lets.contains(p)) || whole_found.iter().any(|w| w.id == f.id);
            // (One about files gone with the change, a removal's, counts while this version does;
            // the next change of version drops it, its files being in it no more.)
            if f.level == Level::Warning && g.accepted.contains(&f.id) && lets_in {
                accepted.insert(f.id.clone(), f.files.clone());
            }
        }
    }
    let changed = files != prev.files || accepted != prev.accepted;
    let index = if changed { Index { fmt: 1, unit: unit.into(), checks: format!("{unit} {}", g.checks.version()), files, accepted } } else { prev.clone() };
    // The sizes and times to compare with next time: a file taken in, or unchanged (touched or
    // not), as listed now; a held change's, or one not held still, as before (read again).
    let mut listed = BTreeMap::new();
    for p in index.files.keys() {
        let changed_held = changes.contains_key(p) && held.contains(p);
        let l = if changed_held || g.unsettled.contains(p) { g.prev_listed.get(p).copied() } else { g.listing.files.get(p).copied().or_else(|| g.prev_listed.get(p).copied()) };
        if let Some(l) = l {
            listed.insert(p.clone(), l);
        }
    }
    let listed_changed = listed != *g.prev_listed;
    // The report.
    let strays: BTreeSet<String> = g.listing.strays.iter().cloned().collect();
    let report = (!held.is_empty() || !strays.is_empty()).then(|| {
        let mut shown: Vec<Finding> = findings.iter().filter(|f| f.holds(g.accepted) && (f.files.iter().any(|p| held.contains(p) || strays.contains(p)) || together.is_some())).cloned().collect();
        shown.sort_by(|a, b| (a.level, &a.files, &a.id).cmp(&(b.level, &b.files, &b.id)));
        shown.dedup_by(|a, b| a.id == b.id);
        let mut raised: Vec<String> = findings.iter().chain(&whole_found).map(|f| f.id.clone()).collect();
        raised.sort();
        raised.dedup();
        Report { fmt: 1, unit: unit.into(), checks: format!("{unit} {}", g.checks.version()), held: held.iter().chain(&strays).cloned().collect(), findings: shown, together, raised }
    });
    Ok(Decision { index, changed, listed, listed_changed, report, store: taken, read })
}

/// A version's bytes, by path: `files`' own, else (None there) the accepted copy's, each read once.
fn reader<'a>(files: &'a BTreeMap<String, Option<Arc<Vec<u8>>>>, prev: &'a Index, read_old: &'a dyn Fn(&FileEntry) -> Result<Vec<u8>>) -> impl Fn(&str) -> Result<Arc<Vec<u8>>> + 'a {
    let cache: std::sync::Mutex<BTreeMap<String, Arc<Vec<u8>>>> = Default::default();
    move |p: &str| -> Result<Arc<Vec<u8>>> {
        if let Some(Some(b)) = files.get(p) {
            return Ok(b.clone());
        }
        if let Some(b) = cache.lock().unwrap().get(p) {
            return Ok(b.clone());
        }
        let e = prev.files.get(p).with_context(|| format!("{p} isn't in the version"))?;
        let b = Arc::new(read_old(e)?);
        cache.lock().unwrap().insert(p.to_string(), b.clone());
        Ok(b)
    }
}

/// The candidate a check uses, from `planned` (what the lead's key was made from: its listing and
/// acceptances), or listed now by itself (by hand). A file whose size or time isn't the listing's
/// now (changed since it was listed), or changed in the last `QUIET_S` (listed here), isn't held
/// still: it's left as accepted, or out while new, and the next listing, which differs, checks it
/// again. So the key recorded never names a file the check didn't take as listed.
pub fn candidate(root: &std::path::Path, unit: &str, recursive: bool, planned: Option<super::Planned>, now: u64) -> Result<(Listing, BTreeSet<String>, BTreeSet<String>)> {
    let raw = super::list(root, unit, recursive)?;
    let (listing, accepted) = match planned {
        Some(p) => (p.listing, p.accepted),
        None => (super::settle(&raw, now), super::acceptances(root, unit)?),
    };
    let mut unsettled: BTreeSet<String> = listing.files.iter().filter(|(p, l)| raw.files.get(*p) != Some(l)).map(|(p, _)| p.clone()).collect();
    // (A file there now that the listing passed over as too recent: not taken as removed.)
    unsettled.extend(raw.files.keys().filter(|p| !listing.files.contains_key(*p)).cloned());
    Ok((listing, accepted, unsettled))
}

/// The job: unit `unit` checked (`full`: every file read; `planned`: the listing and acceptances
/// the lead's key was made from), its outcome handed off with `out`.
/// The records of unit `unit`, off the gate, removed (`scenic inputs test off --forget`); GC sweeps
/// their files once they're old.
pub fn forget(out: &mut crate::out::Out, unit: &str) -> Result<String> {
    anyhow::ensure!(!super::units(out.root()).contains(&unit), "{unit} is on the gate: take it off first");
    let mut gone = Vec::new();
    for l in [super::logical(unit), super::held_logical(unit), super::listed_logical(unit)] {
        if out.get(&l).is_some() {
            out.set(&l, None);
            gone.push(l);
        }
    }
    out.save()?;
    Ok(format!("{unit}: {} record{} forgotten", gone.len(), if gone.len() == 1 { "" } else { "s" }))
}

pub fn run(out: &mut crate::out::Out, unit: &str, full: bool, planned: Option<super::Planned>) -> Result<String> {
    use crate::timings::{phase, Class};
    let checks = super::checks(unit).with_context(|| format!("{unit} isn't a gate unit this app knows"))?;
    let root = out.root().to_path_buf();
    let p = phase("the accepted version and the drop box listed", Class::NasRead);
    let prev_name = out.get(&super::logical(unit)).map(str::to_string);
    let prev = prev_name.as_ref().map(|n| super::read_index(&root, n)).transpose()?;
    let state = super::read_listed(&root, &out.manifest, unit)?;
    let prev_listed = state.listed.clone();
    let (listing, accepted, unsettled) = candidate(&root, unit, checks.recursive(), planned, crate::agent::jobs::now_s())?;
    drop(p);
    let p = phase("the changes read and checked", Class::NasRead);
    let dir = super::drop_box(&root, unit);
    let read_new = |path: &str| std::fs::read(dir.join(path)).with_context(|| format!("read {}", dir.join(path).display()));
    let read_old = |e: &FileEntry| std::fs::read(root.join(&e.file)).with_context(|| format!("read {}", e.file));
    let d = decide(&Given { checks, prev: prev.as_ref(), prev_listed: &prev_listed, listing: &listing, unsettled: &unsettled, accepted: &accepted, full, read_new: &read_new, read_old: &read_old })?;
    p.count(d.store.values().map(|b| b.len() as u64).sum(), d.read.len() as u64);
    drop(p);
    let p = phase("the accepted files, the index and the report stored", Class::NasWrite);
    for (name, bytes) in &d.store {
        let c = store::naming::parse_content_name(name).context("a copy's name")?;
        out.store_bytes(c.logical, c.ext, bytes)?;
    }
    let mut said = format!("{unit}: {} file{} read of {}", d.read.len(), if d.read.len() == 1 { "" } else { "s" }, listing.files.len());
    // (The index a change of version replaces, kept in the check's state: GC keeps it and its files
    // while that state is recent, so the version replaced stays restorable: gc::inputs_kept.)
    let mut replaced = state.replaced.clone();
    if d.changed {
        let name = out.store_bytes(&super::logical(unit), "json", &serde_json::to_vec_pretty(&d.index)?)?;
        if let Some(old) = prev_name.as_ref().filter(|o| **o != name) {
            replaced = Some(old.clone());
            // (And its time made now, for GC's rule by time too; a failure only said.)
            if let Err(e) = std::fs::File::options().write(true).open(root.join(old)).and_then(|f| f.set_modified(std::time::SystemTime::now())) {
                eprintln!("inputs: {unit}: the index replaced, {old}, couldn't be touched ({e}); the check's state keeps it named");
            }
        }
        out.set(&super::logical(unit), Some(name.clone()));
        said += &format!("; taken in: {name}");
    }
    // (A full check's time kept with it: the lead's next is a day after, whichever Mac leads.)
    let full_at = if full { Some(crate::agent::jobs::now_s()) } else { state.full_at };
    if d.listed_changed || replaced != state.replaced || full_at != state.full_at {
        let l = super::Listed { fmt: 1, unit: unit.into(), listed: d.listed.clone(), replaced, full_at };
        let name = out.store_bytes(&super::listed_logical(unit), "json", &serde_json::to_vec_pretty(&l)?)?;
        out.set(&super::listed_logical(unit), Some(name));
    }
    let report = match &d.report {
        Some(r) => Some(out.store_bytes(&super::held_logical(unit), "json", &serde_json::to_vec_pretty(r)?)?),
        None => None,
    };
    if out.get(&super::held_logical(unit)) != report.as_deref() {
        out.set(&super::held_logical(unit), report.clone());
    }
    if let Some(r) = &d.report {
        said += &format!("; held: {} ({} finding{})", r.held.join(", "), r.findings.len(), if r.findings.len() == 1 { "" } else { "s" });
    }
    out.save()?;
    drop(p);
    Ok(said)
}
