//! A job's hand-offs (docs/pool.md §7.3): no job writes the build's records (the manifest, its
//! unverified uploads and the job keys); each saves its changes to a folder of its own instead
//! (crate::out::Out, with `SCENIC_HANDOFF`: its lease's folder), named in the order written (each
//! after the last), and its agent hands them to the pool's journal as the job ends
//! (crate::agent::pool), whose lead merges them into its records. The folders of hand-offs from
//! before the pool (a lead's coordinator's journal on its disk) are drained into the journal the
//! same way, a folder's in order, its last drained marked (`<folder>.merged`, so one that couldn't
//! be removed isn't drained again); one that can't be parsed (it was written whole, so it's damaged)
//! is set aside as `.bad`. What a job uploads is content-named, so two Macs building one target hand
//! off the same names.

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
    /// The sizes (bytes) of the files the job wrote (content name → size), as it wrote them
    /// (crate::out::Out): the records keep them for the catalogs, which then never look at those
    /// files on the NAS. A hand-off from before them has none (the catalog looks at those files once).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sizes: BTreeMap<String, u64>,
    /// A job done: its step, and its targets with their keys (build::Keys::record).
    #[serde(default)]
    pub done: Option<(String, Vec<(String, String)>)>,
    /// AWS's raw terrain tiles the job fetched, packed into archives it put on the NAS (terrain,
    /// peaks: crate::rawpack, a member's own way): (area, archive) for the lead to name in the
    /// raw store's index, which it alone writes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub raw: Vec<(String, crate::rawpack::Pack)>,
}

impl Handoff {
    /// `later` on top of this one (a job's saves in the order written: a later change wins).
    pub fn absorb(&mut self, later: Handoff) {
        self.changes.extend(later.changes);
        self.pending.extend(later.pending);
        self.sizes.extend(later.sizes);
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

/// The folder, under a base, of raw tiles' archives to name (`Handoff::raw` alone): a merge's that
/// couldn't be named then, and those of a hand-off the coordinator didn't take (crate::coord).
pub const RAW_AGAIN: &str = "raw-tiles";

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
        Ok(rd) => rd
            .map(|e| e.map(|e| e.path()))
            .collect::<std::io::Result<Vec<_>>>()
            .with_context(|| format!("list {}", dir.display())),
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
    let mut files: Vec<PathBuf> = listing(dir)?
        .into_iter()
        .filter(|p| {
            p.extension().is_some_and(|x| x == "json")
                && !crate::whole::is_tmp(p)
                && p.file_name().is_some_and(|n| stamp(&n.to_string_lossy()).is_some())
        })
        .collect();
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

/// Every hand-off waiting under `base` (the coordinator's journal of hand-offs received over HTTP,
/// crate::coord, kept on this Mac's disk), a folder's in order, with its path; an error when a
/// listing, a marker or a hand-off can't be read now. One that can't be parsed is set aside; its
/// folder's done records after it in this look are dropped (their units are built again), as its
/// job's saves may be lost with it.
pub fn waiting_in(base: &Path) -> Result<Vec<(PathBuf, Handoff)>> {
    let mut out = Vec::new();
    let mut hosts: Vec<PathBuf> = listing(base)?.into_iter().filter(|p| p.is_dir()).collect();
    hosts.sort();
    for h in hosts {
        let mut files: Vec<PathBuf> = listing(&h)?
            .into_iter()
            .filter(|p| p.extension().is_some_and(|x| x == "json") && !crate::whole::is_tmp(p))
            .collect();
        let done = merged(&h)?;
        files.retain(|p| {
            done.as_deref().is_none_or(|d| p.file_name().is_some_and(|n| n.to_string_lossy().as_ref() > d))
        });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folders_hand_offs_wait_in_order_and_a_damaged_one_is_set_aside() {
        let d = tempfile::tempdir().unwrap();
        let base = d.path().join("journal");
        let m1 = base.join("m1");
        // A job's two saves, then its done record.
        let save = |changes: &[(&str, Option<&str>)]| Handoff {
            changes: changes.iter().map(|(k, v)| (k.to_string(), v.map(str::to_string))).collect(),
            ..Default::default()
        };
        write(&m1, &save(&[("base/6-1-1", Some("base/6-1-1.3333333333333333.base"))])).unwrap();
        write(&m1, &save(&[("base/6-1-1", Some("base/6-1-1.4444444444444444.base")), ("base/6-1-2", None)])).unwrap();
        write(&m1, &Handoff { done: Some(("unit".into(), vec![("6/1/1".into(), "k1".into())])), ..Default::default() })
            .unwrap();
        // A file being written (a temporary name) waits.
        std::fs::write(m1.join("99999999999999999999-1.json.m1.7.tmp"), b"{").unwrap();
        let w = waiting_in(&base).unwrap();
        assert_eq!(w.len(), 3);
        assert!(w.windows(2).all(|p| p[0].0 < p[1].0) && w[2].1.done.is_some(), "in the order written");
        // Marked drained up to the last: none waits; a hand-off left behind isn't drained again; a
        // new one is named after it, whatever the clock; a damaged one is set aside.
        let last = w[2].0.file_name().unwrap().to_string_lossy().into_owned();
        crate::whole::write(&m1.with_extension("merged"), last.as_bytes()).unwrap();
        assert!(waiting_in(&base).unwrap().is_empty());
        let damaged = format!("{:020}-9.json", stamp(&last).unwrap() + 5);
        std::fs::write(m1.join(&damaged), b"{damaged").unwrap();
        write(&m1, &save(&[("base/6-1-3", Some("base/6-1-3.6666666666666666.base"))])).unwrap();
        let w = waiting_in(&base).unwrap();
        assert_eq!(w.len(), 1);
        assert!(w[0].0.file_name().unwrap().to_string_lossy().as_ref() > damaged.as_str());
        assert!(!m1.join(&damaged).exists() && m1.join(&damaged).with_extension("bad").exists());
    }
}
