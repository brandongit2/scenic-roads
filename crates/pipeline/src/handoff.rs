//! A helper's hand-offs (docs/plan.md §8, Two Macs). The build Mac alone writes the build's records
//! (its agent and its jobs): the manifest, its unverified uploads (`pending.json`) and the job keys
//! (`jobs.json`). A
//! helper's job saves its changes to `state/build/handoff/<host>/` instead (crate::out::Out, with
//! `SCENIC_HANDOFF`), and its agent adds a done record there when the job succeeds. The build Mac's
//! agent merges them, a Mac's in the order they were written (by name: each named after the last),
//! records the last it merged (`<host>.merged`, so one it couldn't delete isn't merged again), then
//! deletes them; until then both agents plan with the done records on top of the keys
//! (`build::Keys::load_with_handoffs`). One that can't be parsed (it was written whole, so it's
//! damaged) is set aside as `.bad`. What a helper uploads is content-named, so a unit both Macs
//! built hands off the same names.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Handoff {
    /// Manifest changes: logical name → content name (None: removed).
    #[serde(default)]
    pub changes: BTreeMap<String, Option<String>>,
    /// Uploads not yet verified on the NAS (content name → SHA-256), and those checked.
    #[serde(default)]
    pub pending: BTreeMap<String, String>,
    #[serde(default)]
    pub checked: Vec<String>,
    /// A job done: its step, and its targets with their keys (build::Keys::record).
    #[serde(default)]
    pub done: Option<(String, Vec<(String, String)>)>,
    /// AWS's raw terrain tiles the job fetched, packed into archives it put on the NAS (terrain,
    /// peaks: crate::rawpack, a helper's own way): (area, archive) for the build Mac to name in the
    /// raw store's index, which it alone writes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub raw: Vec<(String, crate::rawpack::Pack)>,
}

impl Handoff {
    /// `later` on top of this one (a job's saves in the order written: a later change wins).
    pub fn absorb(&mut self, later: Handoff) {
        self.changes.extend(later.changes);
        self.pending.extend(later.pending);
        for c in later.checked {
            if !self.checked.contains(&c) {
                self.checked.push(c);
            }
        }
        if later.done.is_some() {
            self.done = later.done;
        }
        for r in later.raw {
            if !self.raw.contains(&r) {
                self.raw.push(r);
            }
        }
    }
}

/// Where hand-offs are written through the NAS, a folder per host.
pub fn nas_base(root: &Path) -> PathBuf {
    root.join("state/build/handoff")
}

/// Where host `host`'s hand-offs go on the NAS.
pub fn dir(root: &Path, host: &str) -> PathBuf {
    nas_base(root).join(host)
}

/// The last hand-off of host folder `dir` merged (its name), if any; an error when it can't be read
/// now.
fn merged(dir: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(dir.with_extension("merged")) {
        Ok(s) => Ok(Some(s.trim().to_string()).filter(|s| !s.is_empty())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read {}", dir.with_extension("merged").display())),
    }
}

/// The files in `dir`, listed whole (an error now is an error, not a shorter list); none when it
/// isn't there.
fn listing(dir: &Path) -> Result<Vec<PathBuf>> {
    match std::fs::read_dir(dir) {
        Ok(rd) => rd.map(|e| e.map(|e| e.path())).collect::<std::io::Result<Vec<_>>>().with_context(|| format!("list {}", dir.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("list {}", dir.display())),
    }
}

/// The time part of a hand-off's name.
fn stamp(name: &str) -> Option<u128> {
    name.split('-').next()?.parse().ok()
}

/// The hand-offs a job wrote into `dir` (a helper's outbox folder: named by time, as `write` names
/// them), in the order written; None when one is damaged (its saves can't all go back: its units are
/// built again).
pub fn written_in(dir: &Path) -> Result<Option<Vec<Handoff>>> {
    let mut files: Vec<PathBuf> = listing(dir)?.into_iter().filter(|p| p.extension().is_some_and(|x| x == "json") && !crate::whole::is_tmp(p) && p.file_name().is_some_and(|n| stamp(&n.to_string_lossy()).is_some())).collect();
    files.sort();
    let mut out = Vec::new();
    for f in files {
        let b = std::fs::read(&f).with_context(|| format!("read {}", f.display()))?;
        match serde_json::from_slice::<Handoff>(&b) {
            Ok(h) => out.push(h),
            Err(e) => {
                eprintln!("handoff: {} can't be parsed ({e})", f.display());
                return Ok(None);
            }
        }
    }
    Ok(Some(out))
}

/// Writes `h` as the next hand-off in `dir`, named by the time (ns) and the process, so a Mac's sort
/// in the order they were written: after every one there and the last merged, whatever the clock
/// says (it may step back).
pub fn write(dir: &Path, h: &Handoff) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    // (The listing first, then the marker: a merge meanwhile writes its marker before it deletes.)
    let names: Vec<u128> = listing(dir)?.iter().filter_map(|p| stamp(&p.file_name()?.to_string_lossy())).collect();
    let seen = names.into_iter().chain(merged(dir)?.as_deref().and_then(stamp)).max();
    let t = seen.map_or(now, |s| now.max(s + 1));
    crate::whole::write(&dir.join(format!("{t:020}-{}.json", std::process::id())), &serde_json::to_vec(h)?)
}

/// Every hand-off waiting (not merged yet), a Mac's in order, with its path; an error when a listing,
/// a marker or a hand-off can't be read now (the NAS is away: nothing merged or planned on a part).
/// One that can't be parsed is set aside; its Mac's done records after it in this look are dropped
/// (their units are built again), as its job's saves may be lost with it.
pub fn waiting(root: &Path) -> Result<Vec<(PathBuf, Handoff)>> {
    waiting_in(&nas_base(root))
}

/// `waiting` for the host folders under `base`: the NAS's, or the coordinator's journal of
/// hand-offs received over HTTP (crate::coord), kept on this Mac's disk.
pub fn waiting_in(base: &Path) -> Result<Vec<(PathBuf, Handoff)>> {
    let mut out = Vec::new();
    let mut hosts: Vec<PathBuf> = listing(base)?.into_iter().filter(|p| p.is_dir()).collect();
    hosts.sort();
    for h in hosts {
        let mut files: Vec<PathBuf> = listing(&h)?.into_iter().filter(|p| p.extension().is_some_and(|x| x == "json") && !crate::whole::is_tmp(p)).collect();
        let done = merged(&h)?;
        files.retain(|p| done.as_deref().is_none_or(|d| p.file_name().is_some_and(|n| n.to_string_lossy().as_ref() > d)));
        files.sort();
        let mut set_aside = false;
        for f in files {
            let b = std::fs::read(&f).with_context(|| format!("read {}", f.display()))?;
            match serde_json::from_slice::<Handoff>(&b) {
                Ok(mut x) => {
                    if set_aside && x.done.take().is_some() {
                        eprintln!("handoff: {}'s done record dropped (a hand-off before it was set aside): its units are built again", f.display());
                    }
                    out.push((f, x));
                }
                Err(e) => {
                    eprintln!("handoff: {} can't be parsed ({e}); set aside", f.display());
                    std::fs::rename(&f, f.with_extension("bad")).ok();
                    set_aside = true;
                }
            }
        }
    }
    Ok(out)
}

/// Merges every waiting hand-off into the build's records, then deletes them; how many. The build
/// Mac's agent, under this Mac's build lock: when another holds it (a job saving, or paused while
/// it held it), none now.
pub fn merge(root: &Path, scratch: &Path) -> Result<usize> {
    merge_from(root, scratch, &[nas_base(root)])
}

/// `merge` of the hand-offs under each of `bases` (the NAS's, the coordinator's journal), all in one
/// save of the records.
pub fn merge_from(root: &Path, scratch: &Path, bases: &[PathBuf]) -> Result<usize> {
    let mut hs = Vec::new();
    for b in bases {
        hs.extend(waiting_in(b)?);
    }
    if hs.is_empty() {
        return Ok(0);
    }
    let Some(lock) = crate::out::BuildLock::try_take(root)? else { return Ok(0) };
    // (Records that can't be read now: an error, and nothing written.)
    let mut out = crate::out::Out::open(root, scratch)?;
    let mut keys = crate::agent::build::Keys::load_strict(root)?;
    let mut last: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut raw: Vec<(String, crate::rawpack::Pack)> = Vec::new();
    for (p, h) in &hs {
        out.absorb(h);
        if let Some((step, targets)) = &h.done {
            keys.record(step, targets);
        }
        raw.extend(h.raw.iter().cloned());
        if let (Some(d), Some(n)) = (p.parent(), p.file_name()) {
            last.insert(d.to_path_buf(), n.to_string_lossy().into_owned());
        }
    }
    out.save_held(&lock).context("merge the hand-offs into the manifest")?;
    keys.save(root).context("merge the hand-offs into the job keys")?;
    // (The raw tiles' archives a helper put on the NAS, named in the raw store's index.)
    crate::rawpack::name_handed(&root.join("sources/aws-terrarium"), &raw, &lock).context("name a helper's raw tiles' archives")?;
    for (d, n) in &last {
        crate::whole::write(&d.with_extension("merged"), n.as_bytes())?;
    }
    for (p, _) in &hs {
        if let Err(e) = std::fs::remove_file(p) {
            eprintln!("handoff: {} merged, but not deleted ({e}); passed over from now on", p.display());
        }
    }
    Ok(hs.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_helpers_saves_and_records_merge_in_order() {
        let d = tempfile::tempdir().unwrap();
        let (root, scratch) = (d.path().join("root"), d.path().join("scratch"));
        std::fs::create_dir_all(root.join("state/build")).unwrap();
        std::fs::write(root.join("state/build/manifest.json"), br#"{"base/6-1-1": "base/6-1-1.1111111111111111.base", "base/6-1-2": "base/6-1-2.2222222222222222.base"}"#).unwrap();
        let m1 = dir(&root, "m1");
        // A job's two saves, then its done record.
        let save = |changes: &[(&str, Option<&str>)]| Handoff { changes: changes.iter().map(|(k, v)| (k.to_string(), v.map(str::to_string))).collect(), ..Default::default() };
        write(&m1, &save(&[("base/6-1-1", Some("base/6-1-1.3333333333333333.base"))])).unwrap();
        write(&m1, &save(&[("base/6-1-1", Some("base/6-1-1.4444444444444444.base")), ("base/6-1-2", None)])).unwrap();
        write(&m1, &Handoff { done: Some(("unit".into(), vec![("6/1/1".into(), "k1".into())])), ..Default::default() }).unwrap();
        // A file being written (a temporary name) waits.
        std::fs::write(m1.join("99999999999999999999-1.json.m1.7.tmp"), b"{").unwrap();
        assert_eq!(waiting(&root).unwrap().len(), 3);
        // The agents plan with its record on top of the keys.
        assert_eq!(crate::agent::build::Keys::load_with_handoffs(&root).unwrap().unit.get("6/1/1").map(String::as_str), Some("k1"));
        // A job holding the build lock: nothing merged now.
        let held = crate::out::BuildLock::take(&root).unwrap();
        assert_eq!(merge(&root, &scratch).unwrap(), 0);
        drop(held);
        // (A sibling test's child may hold the lock a moment, between its fork and its exec.)
        let t0 = std::time::Instant::now();
        let mut merged = merge(&root, &scratch).unwrap();
        while merged == 0 && t0.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::sleep(std::time::Duration::from_millis(20));
            merged = merge(&root, &scratch).unwrap();
        }
        assert_eq!(merged, 3);
        let m: BTreeMap<String, String> = serde_json::from_slice(&std::fs::read(root.join("state/build/manifest.json")).unwrap()).unwrap();
        assert_eq!(m.get("base/6-1-1").map(String::as_str), Some("base/6-1-1.4444444444444444.base"), "the later save wins");
        assert!(!m.contains_key("base/6-1-2"));
        assert_eq!(crate::agent::build::Keys::load(&root).unit.get("6/1/1").map(String::as_str), Some("k1"));
        assert!(waiting(&root).unwrap().is_empty());
        // A hand-off merged but not deleted (left behind) isn't merged again; a new one is named
        // after it, whatever the clock; a damaged one is set aside.
        let last = std::fs::read_to_string(m1.with_extension("merged")).unwrap();
        std::fs::write(m1.join(&last), serde_json::to_vec(&save(&[("base/6-1-1", Some("base/6-1-1.5555555555555555.base"))])).unwrap()).unwrap();
        assert!(waiting(&root).unwrap().is_empty());
        let damaged = format!("{:020}-9.json", stamp(&last).unwrap() + 5);
        std::fs::write(m1.join(&damaged), b"{damaged").unwrap();
        write(&m1, &save(&[("base/6-1-3", Some("base/6-1-3.6666666666666666.base"))])).unwrap();
        let w = waiting(&root).unwrap();
        assert_eq!(w.len(), 1);
        assert!(w[0].0.file_name().unwrap().to_string_lossy().as_ref() > damaged.as_str());
        assert!(!m1.join(&damaged).exists() && m1.join(&damaged).with_extension("bad").exists());
        assert!(m1.join(&last).exists(), "the one left behind stays, passed over");
        // The records unreadable now: nothing merged, nothing written.
        std::fs::remove_file(root.join("state/build/jobs.json")).unwrap();
        std::fs::create_dir(root.join("state/build/jobs.json")).unwrap();
        assert!(merge(&root, &scratch).is_err());
        assert_eq!(waiting(&root).unwrap().len(), 1);
    }
}
