//! Which Mac builds which unit (docs/plan.md §8, Two Macs): an agent claims a job's targets before
//! it starts the job (`state/build/claims/<step> <target>` on the NAS, made with create-new, which
//! the share does atomically), keeps its claims fresh while the job runs, and drops them when it
//! ends. A claim not kept fresh for `STALE` (its Mac asleep, away from the NAS, or gone) is free
//! again. Only the steps both Macs run are claimed (`SHARED`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The steps both Macs run: a helper's agent does their jobs as the build Mac's coordinator leases
/// them (crate::coord), in this order of preference (what later steps wait on first).
pub const SHARED: [&str; 8] = ["terrain", "slope", "trees", "unit", "pois", "peaks", "bldprep", "bldtiles"];
/// How long a claim lasts without being kept fresh.
pub const STALE: Duration = Duration::from_secs(15 * 60);

fn dir(root: &Path) -> PathBuf {
    root.join("state/build/claims")
}

fn path(root: &Path, step: &str, target: &str) -> PathBuf {
    dir(root).join(format!("{step} {}", target.replace('/', "-")))
}

/// Whether claim file `p` is fresh (kept within `STALE`). A time ahead of this Mac's clock (the other
/// Mac's runs a little ahead, or the NAS's) is fresh.
fn fresh(p: &Path) -> bool {
    fresh_at(p, SystemTime::now())
}

/// `fresh`, at `now`.
fn fresh_at(p: &Path, now: SystemTime) -> bool {
    std::fs::metadata(p).and_then(|m| m.modified()).ok().is_some_and(|t| now.duration_since(t).map_or(true, |age| age < STALE))
}

/// The targets of `step` another agent holds now (fresh claims not `me`'s).
pub fn others(root: &Path, step: &str, me: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Ok(rd) = std::fs::read_dir(dir(root)) else { return out };
    let prefix = format!("{step} ");
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(t) = name.strip_prefix(&prefix) else { continue };
        if fresh(&e.path()) && std::fs::read_to_string(e.path()).is_ok_and(|who| who != me) {
            out.insert(t.replacen('-', "/", 2));
        }
    }
    out
}

/// Claims `targets` of `step` for `me` (who: "<host> <pid>"); true when all are now ours (a stale
/// claim is taken over), false when another agent holds one, and then none is kept.
pub fn claim(root: &Path, step: &str, targets: &[String], me: &str) -> bool {
    use std::io::Write;
    if std::fs::create_dir_all(dir(root)).is_err() {
        return false;
    }
    let mut got: Vec<&String> = Vec::new();
    for t in targets {
        let p = path(root, step, t);
        let ok = match std::fs::OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(mut f) => f.write_all(me.as_bytes()).is_ok(),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let mine = std::fs::read_to_string(&p).is_ok_and(|who| who == me);
                // A stale claim: taken over, by writing it whole (whoever writes last holds it, and
                // the loser sees it on its next look).
                mine || (!fresh(&p) && std::fs::write(&p, me).is_ok() && std::fs::read_to_string(&p).is_ok_and(|who| who == me))
            }
            Err(_) => false,
        };
        if !ok {
            release(root, step, &got.into_iter().cloned().collect::<Vec<_>>(), me);
            return false;
        }
        got.push(t);
    }
    true
}

/// Whether another agent holds one of `me`'s claims on `targets` now (taken over once it went
/// stale). A claim that's gone is made again; one that can't be read now isn't counted.
pub fn lost(root: &Path, step: &str, targets: &[String], me: &str) -> bool {
    use std::io::Write;
    for t in targets {
        let p = path(root, step, t);
        match std::fs::read_to_string(&p) {
            Ok(who) if who != me => return true,
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Ok(mut f) = std::fs::OpenOptions::new().write(true).create_new(true).open(&p) {
                    f.write_all(me.as_bytes()).ok();
                }
            }
            Err(_) => {}
        }
    }
    false
}

/// Keeps `me`'s claims on `targets` fresh.
pub fn refresh(root: &Path, step: &str, targets: &[String], me: &str) {
    for t in targets {
        let p = path(root, step, t);
        if std::fs::read_to_string(&p).is_ok_and(|who| who == me) {
            if let Ok(f) = std::fs::File::options().append(true).open(&p) {
                f.set_modified(SystemTime::now()).ok();
            }
        }
    }
}

/// Drops the claims this Mac's earlier agents left (`host`'s, not `me`'s: one agent runs per Mac, so
/// those are a stopped or crashed one's), so the other Mac needn't wait them out.
pub fn release_host(root: &Path, host: &str, me: &str) {
    let Ok(rd) = std::fs::read_dir(dir(root)) else { return };
    let prefix = format!("{host} ");
    for e in rd.flatten() {
        if std::fs::read_to_string(e.path()).is_ok_and(|who| who.starts_with(&prefix) && who != me) {
            std::fs::remove_file(e.path()).ok();
        }
    }
}

/// Drops `me`'s claims on `targets`.
pub fn release(root: &Path, step: &str, targets: &[String], me: &str) {
    for t in targets {
        let p = path(root, step, t);
        if std::fs::read_to_string(&p).is_ok_and(|who| who == me) {
            std::fs::remove_file(&p).ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_agent_at_a_time() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        let ts = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(claim(r, "unit", &ts(&["6/31/20", "6/31/21"]), "m4 1"));
        // Another agent sees them held, and can't claim one of them (nor keeps what it got).
        assert_eq!(others(r, "unit", "m1 2"), ["6/31/20".to_string(), "6/31/21".to_string()].into());
        assert!(others(r, "unit", "m4 1").is_empty());
        assert!(!claim(r, "unit", &ts(&["6/30/20", "6/31/21"]), "m1 2"));
        assert!(!path(r, "unit", "6/30/20").exists());
        // Released: free for the other.
        assert!(!lost(r, "unit", &ts(&["6/31/20", "6/31/21"]), "m4 1"));
        release(r, "unit", &ts(&["6/31/20", "6/31/21"]), "m4 1");
        assert!(claim(r, "unit", &ts(&["6/31/21"]), "m1 2"));
        // The first agent's job, still running, finds one taken: lost.
        assert!(lost(r, "unit", &ts(&["6/31/20", "6/31/21"]), "m4 1"));
        // A claim kept a moment ahead of this Mac's clock is fresh, not stale.
        let ahead = path(r, "unit", "6/31/21");
        let now = SystemTime::now();
        std::fs::File::options().append(true).open(&ahead).unwrap().set_modified(now + Duration::from_secs(1)).unwrap();
        assert!(fresh_at(&ahead, now) && !claim(r, "unit", &ts(&["6/31/21"]), "m4 1"));
        // A restarted agent drops its predecessor's claims, not the other Mac's.
        assert!(claim(r, "unit", &ts(&["6/40/20"]), "m4 1"));
        release_host(r, "m4", "m4 9");
        assert!(!path(r, "unit", "6/40/20").exists() && path(r, "unit", "6/31/21").exists());
        // A stale claim is taken over.
        let p = path(r, "unit", "6/31/21");
        std::fs::File::options().append(true).open(&p).unwrap().set_modified(SystemTime::now() - STALE - Duration::from_secs(1)).unwrap();
        assert!(others(r, "unit", "m4 1").is_empty());
        assert!(claim(r, "unit", &ts(&["6/31/21"]), "m4 1"));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "m4 1");
    }
}
