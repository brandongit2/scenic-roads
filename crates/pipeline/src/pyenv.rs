//! The Python steps' environments (docs/building.md §2): one per lock file, shared by every app
//! version whose `dem/` has the same `uv.lock` and `.python-version`, and never inside a version's
//! folder (a published copy stays as published). They live in `<home>/pyenv/<key>/`, beside
//! `<home>/app/`; a checkout's `dem/` (run by hand) keeps its own in the checkout's `target/pyenv/`.
//!
//! An environment is made by `uv sync` into `.tmp-<key>-<pid>/` and renamed into place, so a
//! folder named by its key is always whole; two Macs' or processes' makings at once each make their
//! own, the first rename wins and the other's copy is dropped. The steps run its `bin/python`
//! directly (no `uv run`: nothing re-syncs or changes it once made).

use anyhow::{ensure, Context, Result};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How long an environment no installed version names is kept (a job an old version started may
/// still be about to run it), counted from its last use.
pub const UNUSED_KEEP: Duration = Duration::from_secs(24 * 3600);

/// How long a `.tmp-*` making goes untouched before it's a leftover (uv writes as it goes).
const STALE_MAKING: Duration = Duration::from_secs(6 * 3600);

/// The file whose time says when an environment was last used.
const USED: &str = ".used";

/// The key of a `dem/` folder's environment: the hash of its lock file and Python pin.
pub fn key(dem: &Path) -> Result<String> {
    let mut b = std::fs::read(dem.join("uv.lock")).with_context(|| format!("{}/uv.lock", dem.display()))?;
    b.push(0);
    b.extend(std::fs::read(dem.join(".python-version")).unwrap_or_default());
    Ok(store::naming::hash16(&b))
}

/// Where a `dem/` folder's environments live: `<home>/pyenv` for an app version's
/// (`<home>/app/<version>/dem`), else the checkout's `target/pyenv`.
pub fn root(dem: &Path) -> PathBuf {
    let dem = dem.canonicalize().unwrap_or_else(|_| dem.to_path_buf());
    let app = dem.parent().and_then(Path::parent);
    match app {
        Some(a) if a.file_name().is_some_and(|n| n == "app") => a.parent().unwrap_or(a).join("pyenv"),
        _ => dem.parent().unwrap_or(&dem).join("target/pyenv"),
    }
}

/// The Python of `dem`'s environment, made first if it's missing (needs uv on the PATH then).
pub fn python(dem: &Path) -> Result<PathBuf> {
    Ok(ensure_in(&root(dem), dem)?.join("bin/python"))
}

/// `dem`'s environment in `root`, made if missing; its last use noted.
pub fn ensure_in(root: &Path, dem: &Path) -> Result<PathBuf> {
    let k = key(dem)?;
    let env = root.join(&k);
    if !env.join("bin/python").exists() {
        std::fs::create_dir_all(root)?;
        let tmp = root.join(format!(".tmp-{k}-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let st = store::spawn::status(
            store::spawn::command("uv").current_dir(dem).env("UV_PROJECT_ENVIRONMENT", &tmp).args([
                "sync",
                "--frozen",
                "--no-install-project",
                "--quiet",
            ]),
        )
        .context("uv sync (uv on the PATH?)");
        let st = match st {
            Ok(s) => s,
            Err(e) => {
                std::fs::remove_dir_all(&tmp).ok();
                return Err(e);
            }
        };
        if !st.success() {
            std::fs::remove_dir_all(&tmp).ok();
            anyhow::bail!("uv sync for {} failed: {st}", dem.display());
        }
        // (Another making's rename got there first: its copy is the same lock's, ours dropped.)
        if std::fs::rename(&tmp, &env).is_ok() {
            eprintln!("python environment {k} made for {}", dem.display());
        } else {
            std::fs::remove_dir_all(&tmp).ok();
        }
        ensure!(env.join("bin/python").exists(), "{}: no bin/python after uv sync", env.display());
    }
    std::fs::File::create(env.join(USED))
        .and_then(|f| f.set_modified(SystemTime::now()))
        .with_context(|| format!("{}/{USED}", env.display()))?;
    Ok(env)
}

/// Removes from `<home>/pyenv` the environments no version in `<home>/app` uses and none has used
/// for `keep`, and makings left unfinished. What it removed, by name.
pub fn prune(home: &Path, keep: Duration) -> Vec<String> {
    let root = home.join("pyenv");
    let Ok(rd) = std::fs::read_dir(&root) else { return Vec::new() };
    let used: Vec<String> = std::fs::read_dir(home.join("app"))
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .filter_map(|e| key(&e.path().join("dem")).ok())
                .collect()
        })
        .unwrap_or_default();
    let idle = |p: &Path| {
        std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok())
    };
    let mut gone = Vec::new();
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        let old = if n.starts_with(".tmp-") {
            idle(&e.path()).is_some_and(|d| d >= STALE_MAKING)
        } else {
            !used.contains(&n) && idle(&e.path().join(USED)).or_else(|| idle(&e.path())).is_some_and(|d| d >= keep)
        };
        if old {
            match std::fs::remove_dir_all(e.path()) {
                Ok(()) => gone.push(n),
                Err(err) => eprintln!("pyenv {n}: can't remove it: {err}"),
            }
        }
    }
    gone
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dem(at: &Path, lock: &str) -> PathBuf {
        let d = at.join("dem");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("uv.lock"), lock).unwrap();
        std::fs::write(d.join(".python-version"), "3.14.7\n").unwrap();
        d
    }

    fn age(p: &Path, secs: u64) {
        let f = std::fs::File::open(p).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(secs)).unwrap();
    }

    #[test]
    fn an_app_versions_environments_live_beside_its_versions() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().canonicalize().unwrap();
        let a = dem(&home.join("app/20261001-0000-aaaaaaa"), "lock 1");
        let b = dem(&home.join("app/20261002-0000-bbbbbbb"), "lock 1");
        assert_eq!(root(&a), home.join("pyenv"));
        assert_eq!(key(&a).unwrap(), key(&b).unwrap());
        let c = dem(&home.join("checkout"), "lock 2");
        assert_eq!(root(&c), home.join("checkout/target/pyenv"));
        assert_ne!(key(&a).unwrap(), key(&c).unwrap());
    }

    #[test]
    fn prune_keeps_what_a_version_uses_or_used_lately() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path();
        let live = dem(&home.join("app/20261002-0000-bbbbbbb"), "lock live");
        let pe = home.join("pyenv");
        let mk = |n: &str, used_ago: u64| {
            std::fs::create_dir_all(pe.join(n)).unwrap();
            std::fs::write(pe.join(n).join(USED), b"").unwrap();
            age(&pe.join(n).join(USED), used_ago);
        };
        let k = key(&live).unwrap();
        mk(&k, 9 * 86400);
        mk("0000000000000001", 2 * 86400);
        mk("0000000000000002", 600);
        std::fs::create_dir_all(pe.join(".tmp-x-1")).unwrap();
        age(&pe.join(".tmp-x-1"), 7 * 3600);
        std::fs::create_dir_all(pe.join(".tmp-y-2")).unwrap();
        let mut gone = prune(home, UNUSED_KEEP);
        gone.sort();
        assert_eq!(gone, vec![".tmp-x-1".to_string(), "0000000000000001".into()]);
        assert!(pe.join(&k).exists() && pe.join("0000000000000002").exists() && pe.join(".tmp-y-2").exists());
    }

    /// Made for real with uv (passed over without it): made once into the home's `pyenv/`, nothing
    /// written into the version's `dem/`, a second version with the same lock sharing it, and
    /// several makings at once leaving one whole environment.
    #[test]
    fn made_once_and_shared() {
        if !store::spawn::output(store::spawn::command("uv").arg("--version")).is_ok_and(|o| o.status.success()) {
            eprintln!("uv isn't here: not made");
            return;
        }
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dem");
        let d = tempfile::tempdir().unwrap();
        let home = d.path().canonicalize().unwrap();
        let mut dems = Vec::new();
        for v in ["20261001-0000-aaaaaaa", "20261002-0000-bbbbbbb"] {
            let dd = home.join("app").join(v).join("dem");
            std::fs::create_dir_all(&dd).unwrap();
            for f in ["pyproject.toml", "uv.lock", ".python-version"] {
                std::fs::copy(repo.join(f), dd.join(f)).unwrap();
            }
            dems.push(dd);
        }
        let pys: Vec<PathBuf> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..3)
                .map(|i| {
                    let d = &dems[i % 2];
                    s.spawn(move || python(d))
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap().unwrap()).collect()
        });
        assert!(pys.windows(2).all(|w| w[0] == w[1]));
        let names: Vec<String> = std::fs::read_dir(home.join("pyenv"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![key(&dems[0]).unwrap()]);
        for dd in &dems {
            assert!(!dd.join(".venv").exists());
        }
        let o =
            store::spawn::output(store::spawn::command(&pys[0]).args(["-c", "import numpy, shapely, tqdm"])).unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    }
}
