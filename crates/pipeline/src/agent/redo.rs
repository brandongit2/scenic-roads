//! `scenic redo` (task #780; docs/pool.md §7.3, §11): finished targets marked to be built again.
//!
//! A target's job recorded it done under its key; a run that saved something wrong under the right
//! key (25 Oct: terrain-hi packs saved without their water while the NAS restarted) is never made
//! again by the plan. The owner names the targets; `scenic redo` asks the lead's coordinator
//! (`/work/redo`), whose agent keeps the ask in its folder (`ASKS`) and runs a job of the `redo` step
//! for it, the lead's own. The job changes no file; its done record ("<step> <target>", no key)
//! makes the merge forget those targets' keys (`build::Keys::record`), so the plan offers them again
//! under the key it has, and what reads their outputs follows by content: the new packs' names
//! change the keys of the steps after them. The old files leave the manifest as the new ones replace
//! them, and GC removes them as any replaced file.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// The step's name: its jobs' and their done records'.
pub const STEP: &str = "redo";

/// The lead agent's asks not yet done, in its folder.
pub const ASKS: &str = "redo.json";

/// The most targets one ask names.
pub const MAX_TARGETS: usize = 2000;

/// An ask: `step`'s `targets` to build again.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ask {
    /// Its number (when the coordinator took it, in ms): its job's id, "redo <n>".
    pub n: u64,
    pub step: String,
    pub targets: Vec<String>,
    /// Who asked ("scenic redo on <host>").
    #[serde(default)]
    pub by: String,
}

/// Whether `step`'s targets can be redone: a step whose records keep a key per target.
pub fn redoable(step: &str) -> bool {
    super::build::Keys::default().per_target(step).is_some()
}

/// Whether `t` reads as a target: a tile ("z/x/y") or a polar block ("s/4/x/y").
pub fn valid_target(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= 32
        && t.split('/').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// `step` and `targets` checked as an ask's: the step's current name, the targets deduplicated in
/// order.
pub fn checked(step: &str, targets: &[String]) -> Result<(String, Vec<String>)> {
    let step = super::steps::current(step).to_string();
    if !redoable(&step) {
        bail!("{step} keeps no key per target, so its targets can't be redone");
    }
    if targets.is_empty() || targets.len() > MAX_TARGETS {
        bail!("1 to {MAX_TARGETS} targets, not {}", targets.len());
    }
    if let Some(t) = targets.iter().find(|t| !valid_target(t)) {
        bail!("{t:?} isn't a target");
    }
    let mut seen = std::collections::BTreeSet::new();
    Ok((step, targets.iter().filter(|t| seen.insert(t.as_str())).cloned().collect()))
}

/// A redo job's done target for `step`'s `target`.
pub fn done_target(step: &str, target: &str) -> String {
    format!("{step} {target}")
}

/// What a redo of `step`'s `targets` clears in `keys`: each target with the key recorded for it
/// (None: none, nothing to clear).
pub fn clears<'a>(keys: &'a super::build::Keys, step: &str, targets: &'a [String]) -> Vec<(&'a str, Option<&'a str>)> {
    targets.iter().map(|t| (t.as_str(), keys.recorded(step, t))).collect()
}

/// The lease of the last done record merged for each of `step`'s `targets` in `rec` (None: none):
/// a redo's merge sets it (`Records::apply`, `slots`), and so does a build's, so it changes when the
/// redo lands and stays changed when the build that follows records the key again.
pub fn marks(
    rec: &crate::pool::records::Records,
    step: &str,
    targets: &[String],
) -> Vec<Option<crate::pool::journal::LeaseId>> {
    targets.iter().map(|t| rec.last.get(step).and_then(|m| m.get(t)).copied()).collect()
}

/// Waits until `read` gives records whose marks for `step`'s `targets` all differ from `before`
/// (the redo's merge landed in them), at most `wait`, reading every `every`. An unreadable record
/// is told to `warn` and read again. Whether it landed. Not by whether the keys are gone: the plan
/// offers the targets as the merge lands, and a rebuilt one has its key again within minutes.
pub fn follow(
    read: &mut dyn FnMut() -> Result<crate::pool::records::Records>,
    step: &str,
    targets: &[String],
    before: &[Option<crate::pool::journal::LeaseId>],
    wait: std::time::Duration,
    every: std::time::Duration,
    warn: &mut dyn FnMut(&anyhow::Error),
) -> bool {
    let t = std::time::Instant::now();
    loop {
        match read() {
            Ok(rec) if marks(&rec, step, targets).iter().zip(before).all(|(now, was)| now != was) => return true,
            Ok(_) => {}
            Err(e) => warn(&e),
        }
        if t.elapsed() >= wait {
            return false;
        }
        std::thread::sleep(every);
    }
}

/// The asks in the lead agent's folder `home`, in the order taken; none when there's no file. An
/// error when it can't be read or parsed (the asks kept, not dropped).
pub fn pending(home: &Path) -> Result<Vec<Ask>> {
    let p = home.join(ASKS);
    match std::fs::read(&p) {
        Ok(b) => serde_json::from_slice(&b).with_context(|| format!("parse {}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("read {}", p.display())),
    }
}

fn write(home: &Path, asks: &[Ask]) -> Result<()> {
    let p = home.join(ASKS);
    if asks.is_empty() {
        return match std::fs::remove_file(&p) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(e).with_context(|| format!("remove {}", p.display()))
            }
            _ => Ok(()),
        };
    }
    store::whole::write(&p, &serde_json::to_vec_pretty(asks)?)
}

/// `asks` kept in `home`, after those there (one of the same number once).
pub fn add(home: &Path, asks: Vec<Ask>) -> Result<()> {
    let mut all = pending(home)?;
    for a in asks {
        if !all.iter().any(|b| b.n == a.n) {
            all.push(a);
        }
    }
    write(home, &all)
}

/// Ask `n` done: off the list in `home`.
pub fn finished(home: &Path, n: u64) -> Result<()> {
    let mut all = pending(home)?;
    all.retain(|a| a.n != n);
    write(home, &all)
}

/// The job's id for ask `n`, and the ask's number from such an id.
pub fn job_id(n: u64) -> String {
    format!("{STEP} {n}")
}

pub fn ask_of(id: &str) -> Option<u64> {
    id.strip_prefix("redo ")?.parse().ok()
}

/// The records' lines a redo of `step`'s `targets` would clear, and the files they name that stay
/// served until the targets are built again: what `scenic redo` prints (a dry run, and the job's
/// log).
pub fn said(
    keys: &super::build::Keys,
    manifest: &BTreeMap<String, String>,
    step: &str,
    targets: &[String],
) -> Vec<String> {
    let mut out = Vec::new();
    let mut n = 0;
    for (t, k) in clears(keys, step, targets) {
        match k {
            Some(k) => {
                n += 1;
                let files: Vec<&str> =
                    manifest.iter().filter(|(l, _)| super::steps::saves(step, t, l)).map(|(_, c)| c.as_str()).collect();
                out.push(format!(
                    "{step} {t}: key {k}{}",
                    if files.is_empty() {
                        String::new()
                    } else {
                        format!(" (served until rebuilt: {})", files.join(", "))
                    }
                ));
            }
            None => out.push(format!("{step} {t}: no key recorded, nothing to clear")),
        }
    }
    out.push(format!("{n} of {} target{} to clear", targets.len(), if targets.len() == 1 { "" } else { "s" }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::{journal::LeaseId, records::Records};
    use std::time::Duration;

    /// Task #789: the follow-up of `--run` returns as soon as the redo's merge lands in the records,
    /// also when the targets were built again, keys recorded, before it read (the owner's first run,
    /// 26 Oct: the lead merged in 10 s and rebuilt the tiles within minutes, and the command, waiting
    /// for the keys to be gone, ran out its 15 minutes); and says no when none landed.
    #[test]
    fn the_follow_up_sees_the_merge_land_though_the_keys_are_back() {
        let ts = vec!["6/1/1".to_string(), "6/1/2".to_string()];
        let lease = |n| LeaseId { term: 3, n };
        let mut r = Records { term: 3, ..Default::default() };
        for t in &ts {
            r.keys.record("terrain-hi", &[(t.clone(), "k".into())]);
            r.last.entry("terrain-hi".into()).or_default().insert(t.clone(), lease(1));
        }
        let before = marks(&r, "terrain-hi", &ts);
        let (wait, every) = (Duration::from_millis(50), Duration::from_millis(1));
        let fixed = |r: &Records| {
            let r = r.clone();
            move || Ok(r.clone())
        };
        assert!(!follow(&mut fixed(&r), "terrain-hi", &ts, &before, wait, every, &mut |_| {}), "nothing merged yet");
        // The redo merges (keys forgotten), then the plan builds each target again (key back).
        let mut reads = 0;
        let mut read = || {
            reads += 1;
            let mut x = r.clone();
            let done: Vec<_> = ts.iter().map(|t| (done_target("terrain-hi", t), String::new())).collect();
            if reads >= 2 {
                x.keys.record(STEP, &done);
                for t in &ts {
                    x.last.get_mut("terrain-hi").unwrap().insert(t.clone(), lease(5));
                }
            }
            if reads >= 3 {
                for t in &ts {
                    x.keys.record("terrain-hi", &[(t.clone(), "k".into())]);
                    x.last.get_mut("terrain-hi").unwrap().insert(t.clone(), lease(9));
                }
            }
            if reads == 1 {
                Err(anyhow::anyhow!("not readable now"))
            } else {
                Ok(x)
            }
        };
        let mut warned = 0;
        assert!(follow(&mut read, "terrain-hi", &ts, &before, Duration::from_secs(5), every, &mut |_| warned += 1));
        assert_eq!((reads, warned), (2, 1), "returned at the first read holding the merge");
        // Only one of two targets changed: not yet.
        let mut part = r.clone();
        part.last.get_mut("terrain-hi").unwrap().insert("6/1/1".into(), lease(5));
        assert!(!follow(&mut fixed(&part), "terrain-hi", &ts, &before, wait, every, &mut |_| {}));
        // Rebuilt before the first read: the keys are back, the marks changed.
        let mut built = r.clone();
        for t in &ts {
            built.last.get_mut("terrain-hi").unwrap().insert(t.clone(), lease(9));
        }
        assert!(follow(&mut fixed(&built), "terrain-hi", &ts, &before, wait, every, &mut |_| {}));
    }
}
