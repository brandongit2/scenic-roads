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
