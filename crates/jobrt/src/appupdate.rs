//! The app on this Mac (docs/macs.md, The updater): every Mac runs it from a local copy of the
//! published app (`<home>/app/<version>/`, `<home>/app/current` pointing at it), and keeps the
//! releases it came from (`Releases`, `<home>/app/releases/`): the live release's archive and
//! manifest and the one before it, as the lead holds them. The lead's coordinator serves its own
//! (docs/coordinator.md §10), and a Mac that becomes the lead already holds them.
//!
//! `check` asks a `Source` (the lead: coordwire::releases) for its live release and the one
//! before; for a version this Mac lacks it fetches the archive into its releases, checks it by its
//! SHA-256, unpacks it and checks every file unpacked by its own; then `current` is pointed at it,
//! and the program that checked restarts into it by its own rule (the agent by its update drain, the
//! map server once the map has been idle a minute). A copy is complete before it's used: it's
//! unpacked in a folder of the copier's own, `<version>.<pid>.tmp/`, whose app is renamed into
//! place. While the lead holds no release (none published to it yet), there is nothing to do.
//!
//! Every Mac's agent checks (coordwire::releases::spawn, from `scenic agent`); the Mac mini's map
//! server checks too (crates/server/src/updater.rs), so the map updates while the agent there is
//! stopped, as long as the lead's coordinator answers (on the lead itself it's the agent's: there
//! the update waits for it, as every Mac's does while the lead is down). One check at a time per Mac: each holds `app/update.lock` (taken without waiting; held,
//! the check is skipped). A version a program runs from is never pruned (`running_versions`).

use anyhow::{bail, ensure, Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// A Mac's releases, under its app's home.
pub const RELEASES: &str = "app/releases";

/// A published version's manifest (tools/app/lib.zsh's app_manifest): every file's SHA-256 and the
/// archive's.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub version: String,
    /// Each file's SHA-256 (hex), by its path in the version's folder: the version's files, all of
    /// them, each checked once unpacked.
    pub sha256: BTreeMap<String, String>,
    /// The SHA-256 (hex) of the version's archive (`<version>.tgz`), checked before it's unpacked.
    pub archive_sha256: String,
}

/// The live release and the one before it (to roll back to), as a source or a Mac holds them.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Live {
    pub current: Option<Manifest>,
    pub previous: Option<Manifest>,
}

impl Live {
    /// The two versions' names.
    pub fn versions(&self) -> (Option<&str>, Option<&str>) {
        (self.current.as_ref().map(|m| m.version.as_str()), self.previous.as_ref().map(|m| m.version.as_str()))
    }
}

/// A version's name, as publish.sh makes it (`<UTC time>-<commit>`): never a path.
pub fn valid_version(v: &str) -> bool {
    v.bytes().next().is_some_and(|b| b.is_ascii_digit()) && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// A file's SHA-256, as a manifest says it (hex).
pub fn sha256(b: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(b))
}

/// Where a check gets releases: the lead (coordwire::releases).
pub trait Source {
    /// Its live release and the one before; None while it holds none.
    fn live(&self) -> Result<Option<Live>>;
    /// Version `v`'s archive, written to `to`.
    fn fetch(&self, v: &str, to: &Path) -> Result<()>;
}

/// How long a `<version>.tmp/` folder or a release's upload goes untouched before it's a leftover:
/// no copy in progress (an updater's, an upload, or `tools/app/install.sh`'s) pauses that long.
const STALE_COPY: Duration = Duration::from_secs(3600);

/// A Mac's releases (`<home>/app/releases/`): `live.json` (the live release's manifest and the one
/// before's, one file written whole), each one's archive (`<version>.tgz`), and `incoming/`, the
/// archives being fetched or uploaded. The lead's coordinator writes it as a publish or a rollback
/// goes in (crate users: agentcore's coord::releases), every Mac's updater as it follows the lead;
/// each change under `lock` and only from what it was read as (`commit`).
pub struct Releases {
    dir: PathBuf,
}

impl Releases {
    pub fn at(dir: PathBuf) -> Releases {
        Releases { dir }
    }

    /// The releases of the app whose home is `home`.
    pub fn of_home(home: &Path) -> Releases {
        Releases::at(home.join(RELEASES))
    }

    /// What it holds live (nothing yet: none).
    pub fn live(&self) -> Result<Live> {
        match std::fs::read(self.dir.join("live.json")) {
            Ok(b) => serde_json::from_slice(&b).context("app/releases/live.json"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Live::default()),
            Err(e) => Err(e).context("app/releases/live.json"),
        }
    }

    /// Version `v`'s archive here.
    pub fn archive(&self, v: &str) -> PathBuf {
        self.dir.join(format!("{v}.tgz"))
    }

    /// The archive of `m`'s release, when this holds it (live, with that SHA-256: checked as it
    /// came in).
    pub fn holds(&self, m: &Manifest) -> Option<PathBuf> {
        let live = self.live().ok()?;
        let named = [live.current, live.previous].into_iter().flatten().any(|l| l == *m);
        Some(self.archive(&m.version)).filter(|p| named && p.is_file())
    }

    /// The folder of the archives coming in.
    pub fn incoming(&self) -> PathBuf {
        self.dir.join("incoming")
    }

    /// A new file in `incoming/` for an archive of version `v` coming in (its name this process's
    /// own and unique).
    pub fn incoming_file(&self, v: &str) -> Result<PathBuf> {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        ensure!(valid_version(v), "not a version's name: {v:?}");
        std::fs::create_dir_all(self.incoming())?;
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(self.incoming().join(format!("{v}.{}.{n}.tgz", std::process::id())))
    }

    /// Changes what it holds live, under its lock: `next(now)` says what to (None: refused, as
    /// things stand), each of `staged` (an archive in `incoming/`, checked) is put in place first,
    /// and both releases' archives must then be here; then `live.json` is written (durable before
    /// it returns) and the archives of neither, and the incoming ones untouched for an hour, are
    /// removed. Ok(next), or Err(now) when refused.
    pub fn commit(
        &self,
        next: impl FnOnce(&Live) -> Option<Live>,
        staged: &[(&str, &Path)],
    ) -> Result<std::result::Result<Live, Live>> {
        std::fs::create_dir_all(&self.dir)?;
        let lock = std::fs::File::options().create(true).truncate(false).write(true).open(self.dir.join("lock"))?;
        lock.lock().context("app/releases/lock")?;
        let now = self.live()?;
        let Some(next) = next(&now) else { return Ok(Err(now)) };
        for (v, p) in staged {
            std::fs::rename(p, self.archive(v)).with_context(|| format!("app {v}: its archive put in place"))?;
        }
        for m in [&next.current, &next.previous].into_iter().flatten() {
            ensure!(self.archive(&m.version).is_file(), "app {}: no archive here", m.version);
        }
        let tmp = self.dir.join(format!("live.json.{}.tmp", std::process::id()));
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&serde_json::to_vec(&next)?)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, self.dir.join("live.json"))?;
        std::fs::File::open(&self.dir)?.sync_all()?;
        let keep: Vec<String> =
            [&next.current, &next.previous].into_iter().flatten().map(|m| format!("{}.tgz", m.version)).collect();
        for e in std::fs::read_dir(&self.dir)?.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.ends_with(".tgz") && !keep.contains(&n) {
                std::fs::remove_file(e.path()).ok();
            }
        }
        for e in std::fs::read_dir(self.incoming()).into_iter().flatten().flatten() {
            let idle =
                e.metadata().and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok());
            if idle.is_some_and(|d| d >= STALE_COPY) {
                std::fs::remove_dir_all(e.path()).or_else(|_| std::fs::remove_file(e.path())).ok();
            }
        }
        Ok(Ok(next))
    }
}

/// An archive checked against its manifest's SHA-256.
pub fn archive_checked(archive: &Path, m: &Manifest) -> Result<()> {
    let got = sha256(&std::fs::read(archive).with_context(|| format!("app {}: its archive", m.version))?);
    ensure!(
        got == m.archive_sha256,
        "app {}: its archive doesn't match its SHA-256 (got {got}, published {})",
        m.version,
        m.archive_sha256
    );
    Ok(())
}

/// The version's archive unpacked into `app` (a new folder; tar keeps the files' modes: the
/// programs' too) and every file checked: the folder unpacked, with the manifest's files alone,
/// each matching its SHA-256.
pub fn unpack_checked(archive: &Path, m: &Manifest, app: &Path) -> Result<()> {
    let v = &m.version;
    std::fs::create_dir_all(app)?;
    // (bsdtar, macOS's, writes only inside `app` without -P: it refuses a path climbing with ".."
    // or through a link, and unpacks an absolute one inside; a link it unpacks is refused below,
    // files_in. The test: an_archive_reaching_outside_its_folder_is_refused.)
    let out = store::spawn::output(store::spawn::command("/usr/bin/tar").arg("-xzf").arg(archive).arg("-C").arg(app))
        .context("run tar")?;
    ensure!(
        out.status.success(),
        "app {v}: tar couldn't unpack its archive: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    let mut found = BTreeMap::new();
    files_in(app, "", &mut found)?;
    for (f, want) in &m.sha256 {
        let Some(p) = found.remove(f) else { bail!("app {v}: {f} isn't in its archive") };
        let got = sha256(&std::fs::read(&p)?);
        ensure!(&got == want, "app {v}: {f} doesn't match its SHA-256 (unpacked {got}, published {want})");
    }
    ensure!(
        found.is_empty(),
        "app {v}: its archive holds files its manifest doesn't: {:?}",
        found.keys().collect::<Vec<_>>()
    );
    Ok(())
}

/// The version the running program was started from (its folder's name under `<home>/app/`; a
/// link through `current` resolves to it), None when it runs from elsewhere (development, tests).
pub fn running_version(home: &Path) -> Option<String> {
    let exe = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok())?;
    let apps = home.join("app").canonicalize().ok()?;
    if !exe.starts_with(&apps) {
        return None;
    }
    exe.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned())
}

/// Files in `incoming/` removed as they go out of scope, unless `commit` put them in place.
struct Staged(Vec<(String, PathBuf)>);

impl Drop for Staged {
    fn drop(&mut self) {
        for (_, p) in &self.0 {
            std::fs::remove_file(p).ok();
        }
    }
}

/// Follows `lead`: its live release made this Mac's, the one before it kept beside it, and
/// `current` pointed at the live one: the version it switched to, None when there's nothing to do
/// (or another check holds the lock, or the lead holds no release). `running` is the caller's own
/// version, kept by the pruning. A lead that doesn't answer is an error: the update waits for it.
pub fn check(home: &Path, lead: &dyn Source, running: &str) -> Result<Option<String>> {
    let dir = home.join("app");
    // (A copy that failed, or a version superseded before its copy finished, leaves its `.tmp`
    // folder; checked at every check, not only after a switch.)
    remove_stale_copies(&dir, STALE_COPY);
    let lock = std::fs::File::create(dir.join("update.lock")).context("app/update.lock")?;
    if lock.try_lock().is_err() {
        return Ok(None);
    }
    let Some(live) = lead.live()? else { return Ok(None) };
    let Some(cur) = live.current.clone() else { return Ok(None) };
    if !valid_version(&cur.version) || live.previous.as_ref().is_some_and(|p| !valid_version(&p.version)) {
        bail!("not a version's name: {:?}", live.versions());
    }
    let releases = Releases::of_home(home);
    let had = releases.live()?;
    let mut staged = Staged(Vec::new());
    let mut obtain = |m: &Manifest| -> Result<PathBuf> {
        if let Some(p) = releases.holds(m) {
            return Ok(p);
        }
        let p = releases.incoming_file(&m.version)?;
        staged.0.push((m.version.clone(), p.clone()));
        lead.fetch(&m.version, &p)?;
        archive_checked(&p, m)?;
        // (On the disk before `commit` names it: a power cut after leaves no torn archive that
        // `holds` would take, unchecked, to unpack or serve as the lead.)
        std::fs::File::open(&p)?.sync_all()?;
        Ok(p)
    };
    let archive = obtain(&cur)?;
    let switched = install(home, &cur, &archive, running)?;
    // (The one before, to roll back to and for this Mac to hold should it lead: not fetched now, it
    // is the next check's to try again, the live one in place meanwhile.)
    let mut next = Live { current: Some(cur), previous: None };
    if let Some(p) = &live.previous {
        match obtain(p) {
            Ok(_) => next.previous = Some(p.clone()),
            Err(e) => eprintln!("app update: the release before, {}: {e:#}", p.version),
        }
    }
    if next != had {
        // (Changed since it was read, the coordinator's or the other updater's here: theirs stands.)
        let staged: Vec<(&str, &Path)> = staged.0.iter().map(|(v, p)| (v.as_str(), p.as_path())).collect();
        releases.commit(|now| (*now == had).then(|| next.clone()), &staged)?.ok();
    }
    Ok(switched)
}

/// Version `m` made this Mac's current app, unpacked from `archive` unless it's here already: the
/// version, None when `current` names it already.
fn install(home: &Path, m: &Manifest, archive: &Path, running: &str) -> Result<Option<String>> {
    let dir = home.join("app");
    let v = &m.version;
    if std::fs::read_link(dir.join("current")).ok().is_some_and(|l| l.as_os_str() == v.as_str()) {
        return Ok(None);
    }
    let dest = dir.join(v);
    if !dest.exists() {
        let tmp = dir.join(format!("{v}.{}.tmp", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let r = unpack_checked(archive, m, &tmp.join("app")).and_then(|()| {
            std::fs::rename(tmp.join("app"), &dest).with_context(|| format!("app {v}: rename its copy"))
        });
        std::fs::remove_dir_all(&tmp).ok();
        r?;
    }
    // Point `current` at it (a new link of this copier's own renamed over the old one).
    let tmp_link = dir.join(format!("current.{}.tmp", std::process::id()));
    std::fs::remove_file(&tmp_link).ok();
    std::os::unix::fs::symlink(v, &tmp_link)?;
    std::fs::rename(&tmp_link, dir.join("current"))?;
    eprintln!("app {v} ready");
    prune(home, v, running);
    Ok(Some(v.clone()))
}

/// Every file in a folder's tree by its path from `dir` (folders followed; anything else, a link
/// among it, refused).
fn files_in(dir: &Path, prefix: &str, out: &mut BTreeMap<String, PathBuf>) -> Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let name = format!("{prefix}{}", e.file_name().to_string_lossy());
        let t = e.file_type()?;
        if t.is_dir() {
            files_in(&e.path(), &format!("{name}/"), out)?;
        } else if t.is_file() {
            out.insert(name, e.path());
        } else {
            bail!("{name}: neither a file nor a folder");
        }
    }
    Ok(())
}

/// Removes app versions nothing needs: keeps the current one, the caller's, those a program on
/// this Mac runs from (`running_versions`: the map server waiting for an idle map, an older
/// server), the build agent's (its jobs run programs from its folder; `agent/status.json`
/// says which, and its `job_apps` those of jobs it took over from an older app's agent), and the
/// newest other one (to roll back to). Versions are named from their UTC publish time, so they
/// sort by age. Unfinished copies (`.tmp`) an hour old go too, and the Python environments no
/// version left uses (crate::pyenv::prune).
pub fn prune(home: &Path, current: &str, running: &str) {
    let dir = home.join("app");
    remove_stale_copies(&dir, STALE_COPY);
    let status: Option<serde_json::Value> =
        std::fs::read(home.join("agent/status.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let agent: Option<String> = status.as_ref().and_then(|v| v["app"].as_str().map(str::to_string));
    // (And those its running jobs run from: jobs an older app's agent started, taken over.)
    let job_apps: Vec<String> = status
        .as_ref()
        .and_then(|v| v["job_apps"].as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let running_here = running_versions(&dir);
    let Ok(rd) = std::fs::read_dir(&dir) else { return };
    let mut versions: Vec<String> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.as_bytes().first().is_some_and(u8::is_ascii_digit) && !n.ends_with(".tmp"))
        .collect();
    versions.sort();
    let mut keep: Vec<&str> = vec![current, running];
    keep.extend(agent.as_deref());
    keep.extend(running_here.iter().map(String::as_str));
    keep.extend(job_apps.iter().map(String::as_str));
    if let Some(v) = versions.iter().rev().find(|v| !keep.contains(&v.as_str())) {
        keep.push(v);
    }
    for v in versions.iter().filter(|v| !keep.contains(&v.as_str())) {
        match std::fs::remove_dir_all(dir.join(v)) {
            Ok(()) => eprintln!("app {v}: removed (replaced)"),
            Err(e) => eprintln!("app {v}: can't remove it: {e}"),
        }
    }
    // (The Python environments the versions left share no longer, a day after their last use.)
    for n in crate::pyenv::prune(home, crate::pyenv::UNUSED_KEEP) {
        eprintln!("python environment {n}: removed (no version uses it)");
    }
}

/// The versions in `dir` (`<home>/app/`) a process of this user runs from: each process's
/// executable's path (the kernel's, links resolved) under `dir/<version>/`.
pub fn running_versions(dir: &Path) -> Vec<String> {
    let Ok(dir) = dir.canonicalize() else { return Vec::new() };
    let mut out: Vec<String> = Vec::new();
    for path in process_paths() {
        if let Ok(rest) = path.strip_prefix(&dir) {
            if let Some(v) = rest.components().next().filter(|_| rest.components().count() > 1) {
                let v = v.as_os_str().to_string_lossy().into_owned();
                if !out.contains(&v) {
                    out.push(v);
                }
            }
        }
    }
    out
}

/// Every process's executable path that can be read (this user's).
fn process_paths() -> Vec<PathBuf> {
    // SAFETY: a null buffer asks for the count; the buffer given after is as long as it says.
    let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if n <= 0 {
        return Vec::new();
    }
    let mut pids = vec![0 as libc::pid_t; n as usize + 64];
    let bytes = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
    // SAFETY: `pids` holds `bytes` bytes.
    let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
    if n <= 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    for &pid in &pids[..(n as usize).min(pids.len())] {
        // SAFETY: `buf` holds as many bytes as it's said to.
        let len = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
        if len > 0 {
            use std::os::unix::ffi::OsStrExt;
            out.push(PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..len as usize])));
        }
    }
    out
}

/// The newest modification time in a folder's tree (the folder itself included).
fn newest_mtime(p: &Path) -> Option<SystemTime> {
    let md = std::fs::symlink_metadata(p).ok()?;
    let mut t = md.modified().ok()?;
    if md.is_dir() {
        for e in std::fs::read_dir(p).ok()?.flatten() {
            if let Some(m) = newest_mtime(&e.path()) {
                t = t.max(m);
            }
        }
    }
    Some(t)
}

/// Removes the half-copied `<version>.tmp/` folders in `<home>/app/` nothing has written to for
/// `age`: an updater's copy that failed or was superseded, or an install that was stopped. A copy
/// under way writes a file at least every few seconds, so it's never one of them.
fn remove_stale_copies(dir: &Path, age: Duration) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !(name.ends_with(".tmp") && name.as_bytes().first().is_some_and(u8::is_ascii_digit)) {
            continue;
        }
        // Folders only (`current.tmp` is a link, and isn't a version's).
        if !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let idle = newest_mtime(&e.path()).and_then(|t| SystemTime::now().duration_since(t).ok());
        if idle.is_some_and(|d| d >= age) {
            match std::fs::remove_dir_all(e.path()) {
                Ok(()) => eprintln!("app {name}: removed (a copy left unfinished)"),
                Err(err) => eprintln!("app {name}: can't remove it: {err}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in lead: a folder holding its live release's manifest (`LIVE`), the one before's
    /// (`previous.json`) and their archives (`<version>.tgz`).
    struct Published<'a>(&'a Path);

    impl Source for Published<'_> {
        fn live(&self) -> Result<Option<Live>> {
            let read =
                |n: &str| -> Option<Manifest> { serde_json::from_slice(&std::fs::read(self.0.join(n)).ok()?).ok() };
            Ok(read(LIVE).map(|c| Live { current: Some(c), previous: read("previous.json") }))
        }

        fn fetch(&self, v: &str, to: &Path) -> Result<()> {
            std::fs::copy(self.0.join(format!("{v}.tgz")), to)
                .map(drop)
                .with_context(|| format!("app {v}: its archive"))
        }
    }

    /// The stand-in lead's live manifest.
    const LIVE: &str = "current.json";

    fn age(p: &Path, secs: u64) {
        let t = SystemTime::now() - Duration::from_secs(secs);
        let f = std::fs::File::options().read(true).open(p).unwrap();
        f.set_modified(t).unwrap();
    }

    #[test]
    fn stale_copies_go_and_live_ones_stay() {
        let d = tempfile::tempdir().unwrap();
        let app = d.path();
        // A copy left two hours ago (the M1's app/20261005-1328-ace4faa.tmp).
        let old = app.join("20261005-1328-ace4faa.tmp");
        std::fs::create_dir_all(old.join("web")).unwrap();
        std::fs::write(old.join("server"), b"half").unwrap();
        std::fs::write(old.join("web/index.html"), b"x").unwrap();
        for p in [old.join("server"), old.join("web/index.html"), old.join("web"), old.clone()] {
            age(&p, 7200);
        }
        // A copy under way: an old folder that got a file a moment ago, deep inside.
        let live = app.join("20261008-0100-abc1234.tmp");
        std::fs::create_dir_all(live.join("dem")).unwrap();
        std::fs::write(live.join("dem/step.py"), b"x").unwrap();
        age(&live.join("dem"), 7200);
        age(&live, 7200);
        // An installed version and the `current.tmp` link are no copies.
        let inst = app.join("20261001-0000-0000000");
        std::fs::create_dir_all(&inst).unwrap();
        age(&inst, 7200);
        std::os::unix::fs::symlink("20261001-0000-0000000", app.join("current.tmp")).unwrap();

        remove_stale_copies(app, STALE_COPY);
        assert!(!old.exists());
        assert!(live.join("dem/step.py").exists());
        assert!(inst.exists());
        assert!(std::fs::symlink_metadata(app.join("current.tmp")).is_ok());
    }

    #[test]
    fn prune_removes_a_stale_copy_and_keeps_the_running_ones() {
        const STAND_IN: &str = "SCENIC_TEST_PRUNE_STAND_IN";
        if std::env::var_os(STAND_IN).is_some() {
            // (The stand-in: this test program run from a version's folder. It says it runs, then
            // runs until its input closes, the test's end or its panic.)
            println!("stand-in running");
            std::io::Read::read_to_end(&mut std::io::stdin(), &mut Vec::new()).ok();
            return;
        }
        // (Not a tempdir: the program run from it is kept, testkit::ran_dir, #1131.)
        let d = store::testkit::ran_dir("appupdate-prune");
        let app = d.join("app");
        for v in ["20261001-0000-aaaaaaa", "20261002-0000-bbbbbbb", "20261003-0000-ccccccc", "20261004-0000-eeeeeee"] {
            std::fs::create_dir_all(app.join(v)).unwrap();
        }
        let tmp = app.join("20261002-1200-ddddddd.tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("server"), b"half").unwrap();
        age(&tmp.join("server"), 7200);
        age(&tmp, 7200);
        // A program still running from the first (the map server waiting for an idle map, or an
        // older server): a copy of this test program run from its folder, this test alone in it as
        // the stand-in (above), running once it says so. (Not a copy of a system program: macOS's
        // launch constraints kill a copy of one of Apple's as it starts, tools/dev/testing.md.)
        let server = app.join("20261001-0000-aaaaaaa/server");
        std::fs::copy(std::env::current_exe().unwrap(), &server).unwrap();
        let mut child = store::spawn::spawn(
            store::spawn::command(&server)
                .args(["--exact", "appupdate::tests::prune_removes_a_stale_copy_and_keeps_the_running_ones"])
                .args(["--nocapture", "--test-threads=1"])
                .env(STAND_IN, "1")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null()),
        )
        .unwrap();
        let mut out =
            std::io::BufRead::lines(std::io::BufReader::new(child.stdout.take().unwrap())).map_while(Result::ok);
        // (On the line the harness began with the test's name: `test … ... stand-in running`.)
        let said = out.by_ref().any(|l| l.ends_with("stand-in running"));
        assert!(said, "the stand-in ended before it ran: {:?}", child.wait());
        assert_eq!(running_versions(&app), vec!["20261001-0000-aaaaaaa".to_string()]);
        prune(&d, "20261004-0000-eeeeeee", "20261004-0000-eeeeeee");
        // (Its end: its input closed, its output read to the end.)
        drop(child.stdin.take());
        out.for_each(drop);
        assert!(child.wait().unwrap().success());
        assert!(!tmp.exists());
        assert!(app.join("20261001-0000-aaaaaaa").exists(), "the running one's");
        assert!(!app.join("20261002-0000-bbbbbbb").exists());
        assert!(app.join("20261003-0000-ccccccc").exists(), "the one to roll back to");
    }

    /// A version published to the stand-in lead as publish.sh publishes it (lib.zsh's app_pack and
    /// app_manifest): its files, the programs among them executable, packed in `<v>.tgz`, and its
    /// manifest made live; `tweak` changes the manifest first.
    fn publish(lead: &Path, v: &str, files: &[(&str, &[u8])], tweak: impl FnOnce(&mut serde_json::Value)) {
        use std::os::unix::fs::PermissionsExt;
        let src = lead.join("staging").join(v);
        for (f, b) in files {
            std::fs::create_dir_all(src.join(f).parent().unwrap()).unwrap();
            std::fs::write(src.join(f), b).unwrap();
            let mode = if b.starts_with(b"#!") { 0o755 } else { 0o644 };
            std::fs::set_permissions(src.join(f), std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let archive = lead.join(format!("{v}.tgz"));
        let st = store::spawn::status(
            store::spawn::command("/usr/bin/tar").arg("-czf").arg(&archive).arg("-C").arg(&src).arg("."),
        )
        .unwrap();
        assert!(st.success());
        let sha: serde_json::Map<String, serde_json::Value> =
            files.iter().map(|(f, b)| (f.to_string(), sha256(b).into())).collect();
        let mut cur = serde_json::json!({"version": v, "sha256": sha, "archive_sha256": sha256(&std::fs::read(&archive).unwrap())});
        tweak(&mut cur);
        std::fs::write(lead.join(LIVE), cur.to_string()).unwrap();
    }

    fn current(home: &Path) -> String {
        std::fs::read_link(home.join("app/current")).unwrap().to_string_lossy().into_owned()
    }

    #[test]
    fn a_new_version_is_unpacked_checked_and_made_current() {
        let (home, lead) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (home, n) = (home.path(), lead.path());
        let a = "20261001-0000-aaaaaaa";
        std::fs::create_dir_all(home.join("app").join(a)).unwrap();
        std::os::unix::fs::symlink(a, home.join("app/current")).unwrap();
        // Nothing published yet: nothing to do.
        assert_eq!(check(home, &Published(n), a).unwrap(), None);
        let v = "20261002-0000-bbbbbbb";
        let app: &[(&str, &[u8])] =
            &[("scenic", b"#!/bin/sh\n"), ("web/index.html", b"x"), ("fonts/Noto Sans Regular/0-255.pbf", b"g")];
        publish(n, v, app, |_| {});
        // Another check holding the lock: skipped.
        let held = std::fs::File::create(home.join("app/update.lock")).unwrap();
        held.lock().unwrap();
        assert_eq!(check(home, &Published(n), a).unwrap(), None);
        drop(held);
        assert_eq!(check(home, &Published(n), a).unwrap().as_deref(), Some(v));
        assert_eq!(current(home), v);
        assert_eq!(std::fs::read(home.join("app").join(v).join("fonts/Noto Sans Regular/0-255.pbf")).unwrap(), b"g");
        use std::os::unix::fs::PermissionsExt;
        let mode = |f: &str| std::fs::metadata(home.join("app").join(v).join(f)).unwrap().permissions().mode();
        assert_eq!(mode("scenic") & 0o111, 0o111, "a program stays one");
        assert_eq!(mode("web/index.html") & 0o111, 0);
        // No copier's folder or link left behind.
        let left: Vec<_> = std::fs::read_dir(home.join("app"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(left.is_empty(), "{left:?}");
        // In place: nothing more to do.
        assert_eq!(check(home, &Published(n), v).unwrap(), None);
        // A version unpacked here already (by the other updater on this Mac): used as it is.
        let u = "20261002-0000-uuuuuuu";
        publish(n, u, &[("scenic", b"#!/bin/sh\n")], |_| {});
        std::fs::create_dir_all(home.join("app").join(u)).unwrap();
        std::fs::write(home.join("app").join(u).join("theirs"), b"").unwrap();
        assert_eq!(check(home, &Published(n), v).unwrap().as_deref(), Some(u));
        assert_eq!(current(home), u);
        assert!(home.join("app").join(u).join("theirs").exists());
        // Refused, `current` left as it was, nothing left behind: an archive that isn't its
        // manifest's; a file that doesn't match its SHA-256; a file the manifest names that the
        // archive lacks; one the archive holds that the manifest doesn't name.
        let w = "20261003-0000-ccccccc";
        let refused: [(&str, Box<dyn FnOnce(&mut serde_json::Value)>); 4] = [
            ("its archive doesn't match", Box::new(|c| c["archive_sha256"] = sha256(b"other").into())),
            ("scenic doesn't match", Box::new(|c| c["sha256"]["scenic"] = sha256(b"other").into())),
            ("dem/step.py isn't in its archive", Box::new(|c| c["sha256"]["dem/step.py"] = sha256(b"").into())),
            (
                "files its manifest doesn't",
                Box::new(|c| drop(c["sha256"].as_object_mut().unwrap().remove("web/index.html"))),
            ),
        ];
        for (why, tweak) in refused {
            publish(n, w, app, tweak);
            let e = format!("{:#}", check(home, &Published(n), u).unwrap_err());
            assert!(e.contains(why), "{why}: {e}");
            assert_eq!(current(home), u);
            assert!(!home.join("app").join(w).exists());
            let left: Vec<_> = std::fs::read_dir(home.join("app"))
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".tmp"))
                .collect();
            assert!(left.is_empty(), "{why}: {left:?}");
        }
        // Once right, it goes in.
        publish(n, w, app, |_| {});
        assert_eq!(check(home, &Published(n), u).unwrap().as_deref(), Some(w));
    }

    /// An archive that would write outside its folder (a path climbing out, an absolute one, a file
    /// through a link it holds, a link out), its SHA-256 the manifest's all the same: refused,
    /// nothing written outside, `current` as it was, nothing left behind.
    #[test]
    fn an_archive_reaching_outside_its_folder_is_refused() {
        let (home, lead, out) =
            (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (home, n, out) = (home.path(), lead.path(), out.path().canonicalize().unwrap());
        let a = "20261001-0000-aaaaaaa";
        std::fs::create_dir_all(home.join("app").join(a)).unwrap();
        std::os::unix::fs::symlink(a, home.join("app/current")).unwrap();
        let src = n.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("f"), b"hi").unwrap();
        std::os::unix::fs::symlink(&out, src.join("l")).unwrap();
        let abs = format!("|^f$|{}/abs|", out.display());
        let cases: [(&str, &[&str]); 4] = [
            ("climbing", &["-s", "|^f$|../../../esc|", "f"]),
            ("absolute", &["-P", "-s", &abs, "f"]),
            ("through a link", &["-s", "|^f$|l/through|", "l", "f"]),
            ("a link", &["l"]),
        ];
        for (i, (what, args)) in cases.into_iter().enumerate() {
            let v = format!("2026100{}-0000-bbbbbbb", i + 2);
            let archive = n.join(format!("{v}.tgz"));
            let st = store::spawn::status(
                store::spawn::command("/usr/bin/tar").arg("-czf").arg(&archive).arg("-C").arg(&src).args(args),
            )
            .unwrap();
            assert!(st.success(), "{what}");
            let cur = serde_json::json!({"version": v, "sha256": {"f": sha256(b"hi")},
                "archive_sha256": sha256(&std::fs::read(&archive).unwrap())});
            std::fs::write(n.join(LIVE), cur.to_string()).unwrap();
            let e = check(home, &Published(n), a);
            assert!(e.is_err(), "{what}: {e:?}");
            assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0, "{what}: written outside");
            assert!(!home.join("esc").exists() && !n.join("esc").exists(), "{what}");
            assert_eq!(current(home), a, "{what}");
            let left: Vec<_> = std::fs::read_dir(home.join("app"))
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|f| f != a && f != "current" && f != "update.lock" && f != "releases")
                .collect();
            assert!(left.is_empty(), "{what}: {left:?}");
        }
    }

    /// A lead that doesn't answer.
    struct Down;
    impl Source for Down {
        fn live(&self) -> Result<Option<Live>> {
            bail!("reach the coordinator: connection refused")
        }
        fn fetch(&self, _: &str, _: &Path) -> Result<()> {
            bail!("reach the coordinator: connection refused")
        }
    }

    /// A lead holding no release (none published to it yet).
    struct HoldsNone;
    impl Source for HoldsNone {
        fn live(&self) -> Result<Option<Live>> {
            Ok(None)
        }
        fn fetch(&self, v: &str, _: &Path) -> Result<()> {
            bail!("{v}: not here")
        }
    }

    /// `publish`, the live manifest kept as the one before first (as the lead does).
    fn publish_after(lead: &Path, v: &str, files: &[(&str, &[u8])]) {
        std::fs::copy(lead.join(LIVE), lead.join("previous.json")).unwrap();
        publish(lead, v, files, |_| {});
    }

    fn held(home: &Path) -> (Option<String>, Option<String>) {
        let l = Releases::of_home(home).live().unwrap();
        let (c, p) = l.versions();
        (c.map(str::to_string), p.map(str::to_string))
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut n: Vec<String> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        n.sort();
        n
    }

    /// A Mac follows its source's releases: each live one fetched, checked, made current and held
    /// with the one before it (what it serves should it lead), the archives of neither removed; a
    /// release it holds already (a rollback's) isn't fetched again.
    #[test]
    fn the_releases_follow_the_source_and_keep_the_one_before() {
        let (home, lead) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (home, n) = (home.path(), lead.path());
        let a = "20261001-0000-aaaaaaa";
        std::fs::create_dir_all(home.join("app").join(a)).unwrap();
        std::os::unix::fs::symlink(a, home.join("app/current")).unwrap();
        let app: &[(&str, &[u8])] = &[("scenic", b"#!/bin/sh\n"), ("web/index.html", b"x")];
        let (v1, v2, v3) = ("20261002-0000-bbbbbbb", "20261003-0000-ccccccc", "20261004-0000-ddddddd");
        publish(n, v1, app, |_| {});
        assert_eq!(check(home, &Published(n), a).unwrap().as_deref(), Some(v1));
        assert_eq!(held(home), (Some(v1.into()), None));
        publish_after(n, v2, app);
        assert_eq!(check(home, &Published(n), v1).unwrap().as_deref(), Some(v2));
        assert_eq!(held(home), (Some(v2.into()), Some(v1.into())));
        publish_after(n, v3, app);
        assert_eq!(check(home, &Published(n), v2).unwrap().as_deref(), Some(v3));
        assert_eq!(held(home), (Some(v3.into()), Some(v2.into())));
        let rel = home.join(RELEASES);
        assert_eq!(
            names(&rel),
            [format!("{v2}.tgz"), format!("{v3}.tgz"), "incoming".into(), "live.json".into(), "lock".into()]
        );
        assert!(names(&rel.join("incoming")).is_empty());
        // Rolled back at the source (its two manifests swapped), its archives gone: both are held.
        let (cur, prev) = (n.join(LIVE), n.join("previous.json"));
        let (c, p) = (std::fs::read(&cur).unwrap(), std::fs::read(&prev).unwrap());
        std::fs::write(&cur, p).unwrap();
        std::fs::write(&prev, c).unwrap();
        for v in [v1, v2, v3] {
            std::fs::remove_file(n.join(format!("{v}.tgz"))).ok();
        }
        assert_eq!(check(home, &Published(n), v3).unwrap().as_deref(), Some(v2));
        assert_eq!(held(home), (Some(v2.into()), Some(v3.into())));
        // In place: nothing to do, nothing changed.
        assert_eq!(check(home, &Published(n), v2).unwrap(), None);
        assert_eq!(held(home), (Some(v2.into()), Some(v3.into())));
    }

    /// A lead that doesn't answer: the update waits (an error, nothing changed). A lead holding
    /// none: nothing to do.
    #[test]
    fn a_lead_down_waits_and_one_holding_none_leaves_the_app() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let a = "20261001-0000-aaaaaaa";
        std::fs::create_dir_all(home.join("app").join(a)).unwrap();
        std::os::unix::fs::symlink(a, home.join("app/current")).unwrap();
        let e = check(home, &Down, a).unwrap_err();
        assert!(format!("{e:#}").contains("connection refused"), "{e:#}");
        assert_eq!(current(home), a);
        assert_eq!(held(home), (None, None));
        assert_eq!(check(home, &HoldsNone, a).unwrap(), None);
        assert_eq!(current(home), a);
        assert_eq!(held(home), (None, None));
    }

    /// A commit goes only from what it was read as: refused, nothing changes and what is live
    /// comes back; an archive it names must be here.
    #[test]
    fn a_commit_goes_only_from_what_it_was_read_as() {
        let d = tempfile::tempdir().unwrap();
        let r = Releases::at(d.path().join("releases"));
        let m = |v: &str| Manifest { version: v.into(), sha256: BTreeMap::new(), archive_sha256: sha256(v.as_bytes()) };
        let (x, y) = ("20261001-0000-aaaaaaa", "20261002-0000-bbbbbbb");
        let one = Live { current: Some(m(x)), previous: None };
        let e = r.commit(|_| Some(one.clone()), &[]).unwrap_err();
        assert!(format!("{e:#}").contains("no archive here"), "{e:#}");
        let staged = r.incoming_file(x).unwrap();
        std::fs::write(&staged, x).unwrap();
        assert_eq!(
            r.commit(|now| now.current.is_none().then(|| one.clone()), &[(x, &staged)]).unwrap(),
            Ok(one.clone())
        );
        assert_eq!(r.live().unwrap(), one);
        // (Read as holding none: it holds x now.)
        let two = Live { current: Some(m(y)), previous: Some(m(x)) };
        assert_eq!(r.commit(|now| now.current.is_none().then(|| two.clone()), &[]).unwrap(), Err(one.clone()));
        assert_eq!(r.live().unwrap(), one);
        assert!(r.holds(&m(x)).is_some() && r.holds(&m(y)).is_none());
    }
}
