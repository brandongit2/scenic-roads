//! The app on this Mac (docs/macs.md, The updater): every Mac runs it from a local copy of the
//! published app (`<home>/app/<version>/`, `<home>/app/current` pointing at it). When the NAS's
//! live manifest (`app/releases/current.json`, `LIVE`) names another version, its archive
//! (`app/releases/<version>.tgz`, one file) is copied here, checked by its SHA-256 and unpacked,
//! and every file unpacked is checked by its own; then `current` is pointed at it, and the program
//! that checked restarts into it by its own rule (the agent by its update drain, the map server
//! once the map has been idle a minute). A copy is complete before it's used: it's unpacked in a
//! folder of the copier's own, `<version>.<pid>.tmp/`, whose app is renamed into place (a copier
//! that finds the version there already drops its own).
//!
//! Every Mac's agent checks (`spawn`, from `scenic agent`); the Mac mini's map server checks too
//! (crates/server/src/updater.rs), so the map updates while the agent there is stopped. One check
//! at a time per Mac: each holds `app/update.lock` (taken without waiting; held, the check is
//! skipped). The servers from before #874 don't know the lock: their copies go to
//! `<version>.tmp/` and their `current` link through `current.tmp`, names this one never uses, so
//! the two never write one folder or link, and a version a program runs from is never pruned
//! (`running_versions`).

use anyhow::{bail, ensure, Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The NAS's manifest of the live version (tools/app/lib.zsh's app_manifest, written last by a
/// publish); `previous.json` beside it is the one before, for `publish.sh --rollback`.
pub const LIVE: &str = "app/releases/current.json";

/// The NAS's manifest of a published version. (Its `files`, the list alone, is for the updaters
/// before #1148, which copied a version's folder file by file: not read here.)
#[derive(serde::Deserialize, Clone, Debug)]
pub struct Current {
    pub version: String,
    /// Each file's SHA-256 (hex), by its path in the version's folder: the version's files, all of
    /// them, each checked once unpacked.
    pub sha256: BTreeMap<String, String>,
    /// The SHA-256 (hex) of the version's archive, `app/releases/<version>.tgz`, checked before
    /// it's unpacked.
    pub archive_sha256: String,
}

/// How long a `<version>.tmp/` folder goes untouched before it's a leftover: no copy in progress
/// (an updater's, or `tools/app/install.sh`'s) pauses that long.
const STALE_COPY: Duration = Duration::from_secs(3600);

fn sha256(b: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(b))
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

/// Copies a newer published app here and points `current` at it: the version it switched to,
/// None when there's nothing to do (or another check holds the lock). `running` is the caller's
/// own version, kept by the pruning.
pub fn check(home: &Path, root: &store::nasfs::NasRoot, running: &str) -> Result<Option<String>> {
    let dir = home.join("app");
    // (A copy that failed, or a version superseded before its copy finished, leaves its `.tmp`
    // folder; checked at every check, NAS or not, not only after a switch.)
    remove_stale_copies(&dir, STALE_COPY);
    let lock = std::fs::File::create(dir.join("update.lock")).context("app/update.lock")?;
    if lock.try_lock().is_err() {
        return Ok(None);
    }
    let Ok(bytes) = root.join(LIVE).read() else { return Ok(None) };
    let cur: Current = serde_json::from_slice(&bytes).context(LIVE)?;
    if cur.version.is_empty() || cur.version.contains('/') {
        return Ok(None);
    }
    // Already in place (this check's earlier, or the other program's).
    if std::fs::read_link(dir.join("current")).ok().is_some_and(|l| l.as_os_str() == cur.version.as_str()) {
        return Ok(None);
    }
    let dest = dir.join(&cur.version);
    if !dest.exists() {
        let tmp = dir.join(format!("{}.{}.tmp", cur.version, std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let r = copy(root, &cur, &tmp);
        // (Another copier, an older server's updater, may have finished first: a rename onto
        // its full folder fails, and its copy is used.)
        let r = r.and_then(|app| match std::fs::rename(&app, &dest) {
            Err(e) if !dest.is_dir() => Err(e).with_context(|| format!("app {}: rename its copy", cur.version)),
            _ => Ok(()),
        });
        std::fs::remove_dir_all(&tmp).ok();
        r?;
    }
    // Point `current` at it (a new link of this copier's own renamed over the old one).
    let tmp_link = dir.join(format!("current.{}.tmp", std::process::id()));
    std::fs::remove_file(&tmp_link).ok();
    std::os::unix::fs::symlink(&cur.version, &tmp_link)?;
    std::fs::rename(&tmp_link, dir.join("current"))?;
    eprintln!("app {} ready", cur.version);
    prune(home, &cur.version, running);
    Ok(Some(cur.version))
}

/// The version's archive copied into `tmp` (a new folder) and checked, then unpacked in
/// `tmp/app` (tar keeps the files' modes: the programs' too) and every file checked: the folder
/// unpacked, with the manifest's files alone, each matching its SHA-256.
fn copy(root: &store::nasfs::NasRoot, cur: &Current, tmp: &Path) -> Result<PathBuf> {
    let v = &cur.version;
    let (archive, app) = (tmp.join("app.tgz"), tmp.join("app"));
    std::fs::create_dir_all(&app)?;
    root.join(format!("app/releases/{v}.tgz"))
        .copy_to_local(&archive)
        .with_context(|| format!("app {v}: its archive"))?;
    let got = sha256(&std::fs::read(&archive)?);
    ensure!(
        got == cur.archive_sha256,
        "app {v}: its archive doesn't match its SHA-256 (copied {got}, published {})",
        cur.archive_sha256
    );
    // (bsdtar, macOS's: it refuses a path that is absolute or climbs with "..".)
    let out = store::spawn::output(store::spawn::command("/usr/bin/tar").arg("-xzf").arg(&archive).arg("-C").arg(&app))
        .context("run tar")?;
    ensure!(
        out.status.success(),
        "app {v}: tar couldn't unpack its archive: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    let mut found = BTreeMap::new();
    files_in(&app, "", &mut found)?;
    for (f, want) in &cur.sha256 {
        let Some(p) = found.remove(f) else { bail!("app {v}: {f} isn't in its archive") };
        let got = sha256(&std::fs::read(&p)?);
        ensure!(&got == want, "app {v}: {f} doesn't match its SHA-256 (unpacked {got}, published {want})");
    }
    ensure!(
        found.is_empty(),
        "app {v}: its archive holds files its manifest doesn't: {:?}",
        found.keys().collect::<Vec<_>>()
    );
    Ok(app)
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

/// Checks every five minutes on a thread of its own, from a program run from an installed
/// version (else nothing): the agent, which restarts into the new version by its update drain.
pub fn spawn(home: PathBuf, root: Option<store::nasfs::NasRoot>) {
    let Some(running) = running_version(&home) else { return };
    std::thread::Builder::new()
        .name("app-update".into())
        .spawn(move || loop {
            if let Some(r) = root.clone().or_else(|| store::nasfs::NasRoot::find(false)) {
                if let Err(e) = check(&home, &r, &running) {
                    eprintln!("app update: {e:#}");
                }
            }
            std::thread::sleep(Duration::from_secs(300));
        })
        .ok();
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

    /// A version published to the stand-in NAS as publish.sh publishes it (lib.zsh's app_pack and
    /// app_manifest): its files, the programs among them executable, packed in
    /// `app/releases/<v>.tgz`, and its manifest made live; `tweak` changes the manifest first.
    fn publish(nas: &Path, v: &str, files: &[(&str, &[u8])], tweak: impl FnOnce(&mut serde_json::Value)) {
        use std::os::unix::fs::PermissionsExt;
        let src = nas.join("staging").join(v);
        for (f, b) in files {
            std::fs::create_dir_all(src.join(f).parent().unwrap()).unwrap();
            std::fs::write(src.join(f), b).unwrap();
            let mode = if b.starts_with(b"#!") { 0o755 } else { 0o644 };
            std::fs::set_permissions(src.join(f), std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let rel = nas.join("app/releases");
        std::fs::create_dir_all(&rel).unwrap();
        let archive = rel.join(format!("{v}.tgz"));
        let st = store::spawn::status(
            store::spawn::command("/usr/bin/tar").arg("-czf").arg(&archive).arg("-C").arg(&src).arg("."),
        )
        .unwrap();
        assert!(st.success());
        let sha: serde_json::Map<String, serde_json::Value> =
            files.iter().map(|(f, b)| (f.to_string(), sha256(b).into())).collect();
        let mut cur = serde_json::json!({"version": v, "files": files.iter().map(|(f, _)| *f).collect::<Vec<_>>(),
            "sha256": sha, "archive_sha256": sha256(&std::fs::read(&archive).unwrap())});
        tweak(&mut cur);
        std::fs::write(nas.join(LIVE), cur.to_string()).unwrap();
    }

    fn current(home: &Path) -> String {
        std::fs::read_link(home.join("app/current")).unwrap().to_string_lossy().into_owned()
    }

    #[test]
    fn a_new_version_is_unpacked_checked_and_made_current() {
        let (home, nas) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (home, n) = (home.path(), nas.path());
        let a = "20261001-0000-aaaaaaa";
        std::fs::create_dir_all(home.join("app").join(a)).unwrap();
        std::os::unix::fs::symlink(a, home.join("app/current")).unwrap();
        let root = store::nasfs::NasRoot::local_for_test(n);
        // Nothing published yet: nothing to do.
        assert_eq!(check(home, &root, a).unwrap(), None);
        let v = "20261002-0000-bbbbbbb";
        let app: &[(&str, &[u8])] =
            &[("scenic", b"#!/bin/sh\n"), ("web/index.html", b"x"), ("fonts/Noto Sans Regular/0-255.pbf", b"g")];
        publish(n, v, app, |_| {});
        // Another check holding the lock: skipped.
        let held = std::fs::File::create(home.join("app/update.lock")).unwrap();
        held.lock().unwrap();
        assert_eq!(check(home, &root, a).unwrap(), None);
        drop(held);
        assert_eq!(check(home, &root, a).unwrap().as_deref(), Some(v));
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
        assert_eq!(check(home, &root, v).unwrap(), None);
        // A version an older server's updater copied first: used as it is, this copy dropped.
        let u = "20261002-0000-uuuuuuu";
        publish(n, u, &[("scenic", b"#!/bin/sh\n")], |_| {});
        std::fs::create_dir_all(home.join("app").join(u)).unwrap();
        std::fs::write(home.join("app").join(u).join("theirs"), b"").unwrap();
        assert_eq!(check(home, &root, v).unwrap().as_deref(), Some(u));
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
            let e = format!("{:#}", check(home, &root, u).unwrap_err());
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
        assert_eq!(check(home, &root, u).unwrap().as_deref(), Some(w));
    }
}
