//! The check job (docs/inputs.md §4.3): one gate unit's drop box against its accepted version.
//!
//! 1. The files whose size or time differ from those the accepted version was taken with are read
//!    and hashed (all of them with `full`); one whose bytes are those accepted is unchanged,
//!    whatever its time.
//! 2. The changed files' shape checks, a removal's, then the unit's own checks over the candidate.
//! 3. Each change's verdict: clean, or held by an unaccepted finding about it; partners held
//!    together, and a file naming a held one held with it.
//! 4. The next version: the accepted one with every clean change applied.
//! 5. That version checked whole: a finding it raises that's neither accepted nor raised by the
//!    accepted version as it stands holds every change of the unit together.
//! 6. The taken files stored content-named under `sources/inputs/<unit>/`, the index and the report
//!    too, and handed off as records changes: `sources/inputs/<unit>/index` (only when it differs)
//!    and `sources/inputs/<unit>/held` (the report, or none when nothing is held).
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
    /// The candidate: the drop box as listed, its files held still.
    pub listing: &'a Listing,
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
        let was = prev.files.get(p);
        if !g.full && was.is_some() && prev.listed.get(p) == Some(&listed) {
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
        if !g.listing.files.contains_key(p) {
            findings.extend(g.checks.removed(p, was));
            changes.insert(p.clone(), Change::Removed);
        }
    }
    // (What isn't the drop box's shape: a folder in a unit without them.)
    for s in &g.listing.strays {
        findings.push(Finding::new("stray", Level::Error, &[s], &[], vec![s.clone()], format!("{s}: {unit} takes no folders (but todo/ and how/): move what's in it up, or out")));
    }
    // The candidate, checked whole.
    let candidate: BTreeMap<String, Option<Arc<Vec<u8>>>> = g
        .listing
        .files
        .keys()
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
    let mut files = prev.files.clone();
    let mut listed = prev.listed.clone();
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
                listed.insert(p.clone(), g.listing.files[p]);
                taken.insert(file, bytes.clone());
            }
            Change::Removed => {
                files.remove(p);
                listed.remove(p);
            }
        }
    }
    // (A file unchanged but touched: its time as listed now, should the version change.)
    for (p, &l) in &g.listing.files {
        if files.contains_key(p) && !changes.contains_key(p) {
            listed.insert(p.clone(), l);
        }
    }
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
            files = prev.files.clone();
            listed = prev.listed.clone();
            taken.clear();
            findings.extend(bad);
        }
    }
    // The index: the accepted warnings it's taken with, kept.
    let mut accepted: BTreeSet<String> = prev.accepted.iter().cloned().collect();
    if files != prev.files {
        let lets: BTreeSet<&String> = changes.keys().filter(|p| !held.contains(*p)).collect();
        for f in findings.iter().chain(&whole_found) {
            if f.level == Level::Warning && g.accepted.contains(&f.id) && (f.files.iter().any(|p| lets.contains(p)) || whole_found.iter().any(|w| w.id == f.id)) {
                accepted.insert(f.id.clone());
            }
        }
    }
    let changed = files != prev.files;
    let index = if changed { Index { fmt: 1, unit: unit.into(), checks: format!("{unit} {}", g.checks.version()), files, listed, accepted: accepted.into_iter().collect() } } else { prev.clone() };
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
    Ok(Decision { index, changed, report, store: taken, read })
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

/// The job: unit `unit` checked (`full`: every file read), its outcome handed off with `out`.
pub fn run(out: &mut crate::out::Out, unit: &str, full: bool) -> Result<String> {
    use crate::timings::{phase, Class};
    let checks = super::checks(unit).with_context(|| format!("{unit} isn't a gate unit this app knows"))?;
    let root = out.root().to_path_buf();
    let p = phase("the accepted version and the drop box listed", Class::NasRead);
    let prev = out.get(&super::logical(unit)).map(|n| super::read_index(&root, n)).transpose()?;
    let raw = super::list(&root, unit, checks.recursive())?;
    // (A file changed in the last QUIET_S is left as accepted, or out while new: the next listing
    // checks it again.)
    let now = crate::agent::jobs::now_s();
    let mut listing = Listing { files: BTreeMap::new(), strays: raw.strays.clone() };
    for (path, &(size, t)) in &raw.files {
        if t + super::QUIET_S <= now {
            listing.files.insert(path.clone(), (size, t));
        } else if let Some(&was) = prev.as_ref().and_then(|i| i.files.contains_key(path).then(|| i.listed.get(path)).flatten()) {
            listing.files.insert(path.clone(), was);
        }
    }
    let accepted = super::acceptances(&root, unit)?;
    drop(p);
    let p = phase("the changes read and checked", Class::NasRead);
    let dir = super::drop_box(&root, unit);
    let read_new = |path: &str| std::fs::read(dir.join(path)).with_context(|| format!("read {}", dir.join(path).display()));
    let read_old = |e: &FileEntry| std::fs::read(root.join(&e.file)).with_context(|| format!("read {}", e.file));
    let d = decide(&Given { checks, prev: prev.as_ref(), listing: &listing, accepted: &accepted, full, read_new: &read_new, read_old: &read_old })?;
    p.count(d.store.values().map(|b| b.len() as u64).sum(), d.read.len() as u64);
    drop(p);
    let p = phase("the accepted files, the index and the report stored", Class::NasWrite);
    for (name, bytes) in &d.store {
        let c = store::naming::parse_content_name(name).context("a copy's name")?;
        out.store_bytes(c.logical, c.ext, bytes)?;
    }
    let mut said = format!("{unit}: {} file{} read of {}", d.read.len(), if d.read.len() == 1 { "" } else { "s" }, listing.files.len());
    if d.changed {
        let name = out.store_bytes(&format!("{}/index", super::store_dir(unit)), "json", &serde_json::to_vec_pretty(&d.index)?)?;
        out.set(&super::logical(unit), Some(name.clone()));
        said += &format!("; taken in: {name}");
    }
    let report = match &d.report {
        Some(r) => Some(out.store_bytes(&format!("{}/held", super::store_dir(unit)), "json", &serde_json::to_vec_pretty(r)?)?),
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
