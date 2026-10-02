//! Running one build job (docs/plan.md §4 and §8): a child process in its own process group, at
//! utility priority (`taskpolicy -c utility`), kept awake on mains power (`caffeinate -s -w`),
//! paused (`SIGSTOP` to the group) while a condition it needs lapses and resumed (`SIGCONT`) when it
//! holds again, and stopped as a group.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// What a job needs to run.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Needs {
    /// Mains power (CPU work).
    pub ac: bool,
    /// The NAS.
    pub nas: bool,
}

/// A job: one step for one unit or pack, or one worldwide step.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobSpec {
    /// Stable id ("osm-pass 2026-09-28", "gc", "backup").
    pub id: String,
    /// What it does, in plain words, for the status ("OpenStreetMap pass (planet of 28 September)").
    pub what: String,
    /// The program and its arguments.
    pub cmd: Vec<String>,
    pub needs: Needs,
    /// Restart from scratch after the Mac slept (a stage that touched the NAS may hold dead SMB
    /// handles). Jobs resume from their completion markers, so this only repeats the current stage.
    pub restart_after_sleep: bool,
}

/// Seconds since the epoch.
pub fn now_s() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// A job running (or paused) under the agent.
pub struct Running {
    pub spec: JobSpec,
    child: Child,
    caffeinate: Option<Child>,
    pub pgid: i32,
    pub started: u64,
    pub log: PathBuf,
    pub paused: Option<String>,
    started_at: Instant,
}

/// The job record kept on disk while a job runs, so an agent started after a crash can stop an
/// orphaned job before running it again.
#[derive(Serialize, Deserialize)]
struct Record {
    id: String,
    pgid: i32,
    started: u64,
}

impl Running {
    /// Starts `spec` with `threads` worker threads (RAYON_NUM_THREADS), its output appended to
    /// `log`, and records it in `record`.
    pub fn start(spec: JobSpec, threads: usize, log: PathBuf, record: &Path) -> Result<Running> {
        if let Some(d) = log.parent() {
            std::fs::create_dir_all(d)?;
        }
        let out = File::options().create(true).append(true).open(&log).with_context(|| format!("open {}", log.display()))?;
        let err = out.try_clone()?;
        let (prog, args) = spec.cmd.split_first().context("empty command")?;
        let mut c = Command::new("/usr/sbin/taskpolicy");
        c.args(["-c", "utility"]).arg(prog).args(args);
        c.env("RAYON_NUM_THREADS", threads.to_string()).stdin(Stdio::null()).stdout(out).stderr(err);
        // Its own process group, so pausing and stopping reach every process it starts.
        c.process_group(0);
        let child = c.spawn().with_context(|| format!("start {}", spec.id))?;
        let pgid = child.id() as i32;
        // Awake while it runs, on mains power only (-s), ending with it (-w).
        let caffeinate = Command::new("/usr/bin/caffeinate").args(["-s", "-w", &child.id().to_string()]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok();
        let started = now_s();
        let rec = Record { id: spec.id.clone(), pgid, started };
        std::fs::write(record, serde_json::to_vec(&rec)?)?;
        Ok(Running { spec, child, caffeinate, pgid, started, log, paused: None, started_at: Instant::now() })
    }

    /// Pauses the job's whole process group (`why` goes to the status).
    pub fn pause(&mut self, why: &str) {
        if self.paused.is_none() {
            // SAFETY: plain syscall on our own child's group.
            unsafe { libc::killpg(self.pgid, libc::SIGSTOP) };
        }
        self.paused = Some(why.to_string());
    }

    pub fn resume(&mut self) {
        if self.paused.take().is_some() {
            // SAFETY: as above.
            unsafe { libc::killpg(self.pgid, libc::SIGCONT) };
        }
    }

    /// Its exit status once it has finished.
    pub fn poll(&mut self) -> Result<Option<ExitStatus>> {
        Ok(self.child.try_wait()?)
    }

    /// Stops the whole group: SIGTERM (after SIGCONT, so a paused job can handle it), then SIGKILL
    /// after `grace`.
    pub fn stop(&mut self, grace: Duration) {
        stop_group(self.pgid, grace, || self.child.try_wait().ok().flatten().is_some());
        let _ = self.child.wait();
        if let Some(c) = self.caffeinate.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(c) = self.caffeinate.as_mut() {
            let _ = c.try_wait();
        }
    }
}

fn stop_group(pgid: i32, grace: Duration, mut gone: impl FnMut() -> bool) {
    // SAFETY: signals to a process group we started.
    unsafe {
        libc::killpg(pgid, libc::SIGCONT);
        libc::killpg(pgid, libc::SIGTERM);
    }
    let t = Instant::now();
    while t.elapsed() < grace {
        if gone() {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    // SAFETY: as above.
    unsafe { libc::killpg(pgid, libc::SIGKILL) };
}

/// Stops a job left running by an agent that ended without stopping it (a crash, a kill), from its
/// record; removes the record.
pub fn stop_orphan(record: &Path) {
    let Ok(b) = std::fs::read(record) else { return };
    if let Ok(r) = serde_json::from_slice::<Record>(&b) {
        // SAFETY: signal 0 only checks that the group exists.
        if r.pgid > 1 && unsafe { libc::killpg(r.pgid, 0) } == 0 {
            eprintln!("agent: stopping {} left running by an earlier agent (group {})", r.id, r.pgid);
            // SAFETY: as above.
            let exists = || unsafe { libc::killpg(r.pgid, 0) } != 0;
            stop_group(r.pgid, Duration::from_secs(30), exists);
        }
    }
    std::fs::remove_file(record).ok();
}

/// The last `n` lines of a log.
pub fn tail(log: &Path, n: usize) -> String {
    let Ok(b) = std::fs::read(log) else { return String::new() };
    let s = String::from_utf8_lossy(&b[b.len().saturating_sub(64 << 10)..]).into_owned();
    // Progress bars redraw with carriage returns: keep each line's last state.
    let lines: Vec<&str> = s.lines().map(|l| l.rsplit('\r').next().unwrap_or(l)).filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(cmd: &[&str]) -> JobSpec {
        JobSpec { id: "t".into(), what: "test".into(), cmd: cmd.iter().map(|s| s.to_string()).collect(), needs: Needs::default(), restart_after_sleep: false }
    }

    #[test]
    fn runs_pauses_and_stops() {
        let d = tempfile::tempdir().unwrap();
        let rec = d.path().join("job.json");
        let mut r = Running::start(spec(&["/bin/sh", "-c", "echo hello; sleep 30"]), 2, d.path().join("log"), &rec).unwrap();
        assert!(rec.exists());
        std::thread::sleep(Duration::from_millis(300));
        r.pause("test");
        assert!(r.poll().unwrap().is_none());
        r.resume();
        r.stop(Duration::from_secs(5));
        assert!(tail(&r.log, 5).contains("hello"));
    }

    #[test]
    fn exit_status() {
        let d = tempfile::tempdir().unwrap();
        let mut r = Running::start(spec(&["/bin/sh", "-c", "exit 3"]), 1, d.path().join("log"), &d.path().join("job.json")).unwrap();
        let st = loop {
            if let Some(s) = r.poll().unwrap() {
                break s;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        assert_eq!(st.code(), Some(3));
    }

    #[test]
    fn orphans_are_stopped() {
        let d = tempfile::tempdir().unwrap();
        let rec = d.path().join("job.json");
        // The shell leaves a sleep behind in its group and exits: an orphan, as after a crash.
        let mut r = Running::start(spec(&["/bin/sh", "-c", "/bin/sleep 60 & echo started"]), 1, d.path().join("log"), &rec).unwrap();
        while r.poll().unwrap().is_none() {
            std::thread::sleep(Duration::from_millis(50));
        }
        let pgid = r.pgid;
        // SAFETY: probe only.
        assert_eq!(unsafe { libc::killpg(pgid, 0) }, 0, "the sleep is still in the group");
        let t = Instant::now();
        stop_orphan(&rec);
        assert!(t.elapsed() < Duration::from_secs(10), "stopped by SIGTERM, not after the grace");
        // SAFETY: probe only.
        assert_ne!(unsafe { libc::killpg(pgid, 0) }, 0);
        assert!(!rec.exists());
    }
}
