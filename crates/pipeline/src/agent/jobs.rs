//! Running one build job (docs/plan.md §4 and §8): a child process in its own process group, at
//! utility priority (`taskpolicy -c utility`), the Mac kept awake while it runs (`caffeinate -i -s
//! -w`: no idle sleep, on battery too, and no sleep on mains power) but not while it's paused
//! (`SIGSTOP` to the group while a condition it needs lapses; `SIGCONT` when it holds again), and
//! stopped as a group.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use crate::sys::Signal;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// What a job needs to run.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Needs {
    /// CPU work: it runs on mains power, or on battery down to `cond::BATTERY_MIN` (30 %), then
    /// waits for mains (`lapsed`). Light work (a backup, GC) runs on any charge.
    #[serde(alias = "ac")]
    pub cpu: bool,
    /// The NAS.
    pub nas: bool,
    /// The home network: the job reads the whole planet (or every piece of it) from the NAS, too
    /// much to read through Tailscale.
    #[serde(default)]
    pub home: bool,
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
    /// A build step's targets and keys, recorded in state/build/jobs.json when it succeeds.
    #[serde(default)]
    pub record: Option<super::build::Work>,
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
    /// Why it's frozen where it is, while it is.
    pub paused: Option<String>,
    /// Why it's stopping at its next safe point (crate::control), while it is.
    pub pausing: Option<String>,
    started_at: Instant,
    /// Its progress when it first reported this kind of progress: the estimate's start (time,
    /// fraction, unit).
    pub progress_base: Option<(Instant, f64, String, Option<usize>)>,
    /// Its parts and the one it's on, as it last said (`part`): kept while a part's own output
    /// pushes that line out of the log's end.
    pub parts: Option<(usize, Vec<String>)>,
}

/// The job record kept on disk while a job runs, so an agent started after a crash can stop an
/// orphaned job before running it again. The group leader's start time tells it from an unrelated
/// process that got the same id after a restart.
#[derive(Serialize, Deserialize)]
struct Record {
    id: String,
    pgid: i32,
    started: u64,
    #[serde(default)]
    leader_start: u64,
}

use crate::sys::{group_members, process_start};

impl Running {
    /// Starts `spec` with `threads` worker threads (RAYON_NUM_THREADS) and `env`, its output
    /// appended to `log`, and records it in `record`.
    pub fn start(spec: JobSpec, threads: usize, env: &[(&str, &str)], log: PathBuf, record: &Path) -> Result<Running> {
        if let Some(d) = log.parent() {
            std::fs::create_dir_all(d)?;
        }
        let mut out = File::options().create(true).append(true).open(&log).with_context(|| format!("open {}", log.display()))?;
        // (A job's log keeps its earlier runs': this run's begins here, and only what follows is
        // its progress, parts and last lines.)
        std::io::Write::write_all(&mut out, format!("{RUN_START}{} ({}) ===\n", spec.id, now_s()).as_bytes())?;
        let err = out.try_clone()?;
        let (prog, args) = spec.cmd.split_first().context("empty command")?;
        let mut c = Command::new("/usr/sbin/taskpolicy");
        c.args(["-c", "utility"]).arg(prog).args(args);
        // (The Python steps' output reaches the log, and so the status, as they print it: piped, it
        // would wait in their buffers, for minutes.)
        c.env("RAYON_NUM_THREADS", threads.to_string()).env("PYTHONUNBUFFERED", "1").envs(env.iter().copied()).stdin(Stdio::null()).stdout(out).stderr(err);
        // Its own process group, so pausing and stopping reach every process it starts.
        crate::sys::own_group(&mut c);
        let child = c.spawn().with_context(|| format!("start {}", spec.id))?;
        let pgid = child.id() as i32;
        let caffeinate = keep_awake(child.id());
        let started = now_s();
        let rec = Record { id: spec.id.clone(), pgid, started, leader_start: process_start(pgid).unwrap_or(0) };
        std::fs::write(record, serde_json::to_vec(&rec)?)?;
        Ok(Running { spec, child, caffeinate, pgid, started, log, paused: None, pausing: None, started_at: Instant::now(), progress_base: None, parts: None })
    }

    /// Pauses the job's whole process group (`why` goes to the status), and lets the Mac sleep.
    pub fn pause(&mut self, why: &str) {
        if self.paused.is_none() {
            crate::sys::signal_group(self.pgid, Signal::Stop);
            if let Some(mut c) = self.caffeinate.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
        self.paused = Some(why.to_string());
    }

    pub fn resume(&mut self) {
        if self.paused.take().is_some() {
            crate::sys::signal_group(self.pgid, Signal::Cont);
            self.caffeinate = keep_awake(self.child.id());
        }
    }

    /// Its exit status once it has finished.
    pub fn poll(&mut self) -> Result<Option<ExitStatus>> {
        Ok(self.child.try_wait()?)
    }

    /// Stops the whole group: SIGTERM (after SIGCONT, so a paused job can handle it), then SIGKILL
    /// after `grace` to whatever of it is left.
    pub fn stop(&mut self, grace: Duration) {
        let child = &mut self.child;
        stop_group(self.pgid, grace, || {
            let _ = child.try_wait();
        });
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

/// Keeps the Mac awake while process `pid` runs: no idle sleep (on battery too: -i), no system sleep
/// on mains power (-s), ending with it (-w).
fn keep_awake(pid: u32) -> Option<Child> {
    Command::new("/usr/bin/caffeinate").args(["-i", "-s", "-w", &pid.to_string()]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok()
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(c) = self.caffeinate.as_mut() {
            let _ = c.try_wait();
        }
    }
}

/// Stops every process of a group: SIGTERM, then SIGKILL after `grace` if any is left. `reap`
/// collects our own exited child, so it doesn't linger in the group as a zombie.
fn stop_group(pgid: i32, grace: Duration, mut reap: impl FnMut()) {
    crate::sys::signal_group(pgid, Signal::Cont);
    crate::sys::signal_group(pgid, Signal::Term);
    let t = Instant::now();
    while t.elapsed() < grace {
        reap();
        if group_members(pgid).is_empty() {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    crate::sys::signal_group(pgid, Signal::Kill);
    reap();
}

/// Stops a job left running by an agent that ended without stopping it (a crash, a kill), from its
/// record; removes the record.
pub fn stop_orphan(record: &Path) {
    let Ok(b) = std::fs::read(record) else { return };
    if let Ok(r) = serde_json::from_slice::<Record>(&b) {
        let members = if r.pgid > 1 { group_members(r.pgid) } else { Vec::new() };
        // Ours when the leader is the process we started; or, the leader gone, when every member
        // started after the job did (a group id isn't reused while any member lives).
        let ours = !members.is_empty()
            && match process_start(r.pgid) {
                Some(t) => r.leader_start != 0 && t == r.leader_start,
                None => members.iter().all(|&p| process_start(p).is_some_and(|t| t + 2 >= r.started)),
            };
        if ours {
            eprintln!("agent: stopping {} left running by an earlier agent (group {})", r.id, r.pgid);
            stop_group(r.pgid, Duration::from_secs(30), || {});
        }
    }
    std::fs::remove_file(record).ok();
}

/// The last `n` lines of a log.
pub fn tail(log: &Path, n: usize) -> String {
    let s = end_of(log);
    // Progress bars redraw with carriage returns: keep each line's last state. (The parts' lines
    // are the status's list, not text to show.)
    let lines: Vec<&str> = s.lines().map(|l| l.rsplit('\r').next().unwrap_or(l)).filter(|l| !l.trim().is_empty() && !l.starts_with("parts: ")).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// The line each run of a job begins with in its log (`Running::start`).
const RUN_START: &str = "=== run of ";

/// A log's last 64 KB (logs grow long: only the end is read), from its last run's start on.
fn end_of(log: &Path) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = File::open(log) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(64 << 10)));
    let mut b = Vec::new();
    let _ = f.read_to_end(&mut b);
    let s = String::from_utf8_lossy(&b).into_owned();
    match s.rfind(&format!("\n{RUN_START}")).map(|i| i + 1).or(s.starts_with(RUN_START).then_some(0)) {
        Some(i) => s[i..].split_once('\n').map_or(String::new(), |(_, rest)| rest.to_string()),
        None => s,
    }
}

/// Says how far a job is, for the agent (`progress`): `progress: <done>/<total> <unit>` on stderr,
/// which goes to the job's log.
pub fn report(done: u64, total: u64, unit: &str) {
    eprintln!("progress: {}/{total} {unit}", done.min(total));
}

/// Says which of a job's parts (more than one, in order) it begins, for the agent (`parts`):
/// `parts: <i> <their names, JSON>` on stderr, the whole list each time, so the status lists them
/// under the job (done, under way, to come) however long ago it began.
pub fn part(i: usize, names: &[&str]) {
    eprintln!("parts: {i} {}", serde_json::to_string(names).unwrap_or_default());
}

/// A job's parts and the one it's on: its log's last `parts:` line (`part`).
pub fn parts(log: &Path) -> Option<(usize, Vec<String>)> {
    end_of(log).lines().rev().find_map(|l| {
        let (i, names) = l.trim().strip_prefix("parts: ")?.split_once(' ')?;
        let names: Vec<String> = serde_json::from_str(names).ok()?;
        let i: usize = i.parse().ok()?;
        (i < names.len()).then_some((i, names))
    })
}

/// How far a job says it is: its log's last `progress: <done>/<total> <unit>` line (build steps
/// print them: scenic-build's `progress`).
pub fn progress(log: &Path) -> Option<(f64, f64, String)> {
    let s = end_of(log);
    for l in s.lines().rev().map(|l| l.rsplit('\r').next().unwrap_or(l).trim()) {
        // (One said before the part under way began, or this run, is another's: none yet.)
        if l.starts_with("parts: ") || l.starts_with("=== run of ") {
            return None;
        }
        let Some(rest) = l.strip_prefix("progress: ") else { continue };
        let (frac, unit) = rest.split_once(' ').unwrap_or((rest, ""));
        let Some((d, t)) = frac.split_once('/') else { continue };
        let (Ok(d), Ok(t)) = (d.parse::<f64>(), t.parse::<f64>()) else { continue };
        if t > 0.0 {
            return Some((d.min(t), t, unit.trim().to_string()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(cmd: &[&str]) -> JobSpec {
        JobSpec { id: "t".into(), what: "test".into(), cmd: cmd.iter().map(|s| s.to_string()).collect(), needs: Needs::default(), restart_after_sleep: false, record: None }
    }

    #[test]
    fn runs_pauses_and_stops() {
        let d = tempfile::tempdir().unwrap();
        let rec = d.path().join("job.json");
        let mut r = Running::start(spec(&["/bin/sh", "-c", "echo hello; sleep 30"]), 2, &[], d.path().join("log"), &rec).unwrap();
        assert!(rec.exists());
        std::thread::sleep(Duration::from_millis(300));
        r.pause("test");
        assert!(r.poll().unwrap().is_none());
        r.resume();
        r.stop(Duration::from_secs(5));
        assert!(tail(&r.log, 5).contains("hello"));
    }

    #[test]
    fn a_job_asked_to_stop_does_at_its_next_safe_point() {
        // A job that builds a target a quarter of a second, noting each done, and stops at the next
        // safe point once its channel says so (crate::control's protocol, as scenic-build's steps).
        let d = tempfile::tempdir().unwrap();
        let (control, done) = (d.path().join("control"), d.path().join("done.txt"));
        std::fs::write(&control, b"run").unwrap();
        let script = r#"for t in 6/1/1 6/1/2 6/1/3 6/1/4 6/1/5 6/1/6 6/1/7 6/1/8; do
            if grep -q drain "$SCENIC_CONTROL"; then exit 75; fi
            sleep 0.25; echo "unit $t" >> "$SCENIC_DONE"
        done"#;
        let env = [(crate::control::CONTROL_ENV, control.to_str().unwrap()), (crate::control::DONE_ENV, done.to_str().unwrap())];
        let mut r = Running::start(spec(&["/bin/sh", "-c", script]), 1, &env, d.path().join("log"), &d.path().join("job.json")).unwrap();
        std::thread::sleep(Duration::from_millis(600));
        std::fs::write(&control, b"drain").unwrap();
        let t = Instant::now();
        let st = loop {
            if let Some(st) = r.poll().unwrap() {
                break st;
            }
            assert!(t.elapsed() < Duration::from_secs(5), "it stopped at its next safe point");
            std::thread::sleep(Duration::from_millis(50));
        };
        assert_eq!(st.code(), Some(crate::control::PAUSED_EXIT));
        let finished = crate::control::read_done(&done, "unit");
        assert!((2..=4).contains(&finished.len()) && finished[0] == "6/1/1", "{finished:?}");
    }

    #[test]
    fn a_jobs_parts_are_read_from_its_log() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("log");
        assert_eq!(parts(&log), None);
        // As a step prints them (pipeline::agent::jobs::part), with its output between.
        let names = ["Getting ready (osmium)", "Details: Wikidata's facts", "Uploading"];
        let line = |i: usize| format!("parts: {i} {}\n", serde_json::to_string(&names).unwrap());
        std::fs::write(&log, format!("{}progress: 0/3 parts (Getting ready)\nosmium: done\n{}some output\n", line(0), line(1))).unwrap();
        assert_eq!(parts(&log), Some((1, names.iter().map(|s| s.to_string()).collect())));
        // A run after it: none of the earlier run's.
        let mut f = File::options().append(true).open(&log).unwrap();
        std::io::Write::write_all(&mut f, format!("{RUN_START}t (1) ===\nstarting\n").as_bytes()).unwrap();
        assert_eq!((parts(&log), progress(&log), tail(&log, 5).as_str()), (None, None, "starting"));
        std::fs::write(&log, format!("{}progress: 0/3 parts (Getting ready)\nosmium: done\n{}some output\n", line(0), line(1))).unwrap();
        // Not text to show; and a line out of range is none.
        assert_eq!(tail(&log, 5), "progress: 0/3 parts (Getting ready)\nosmium: done\nsome output");
        std::fs::write(&log, format!("parts: 7 {}\n", serde_json::to_string(&names).unwrap())).unwrap();
        assert_eq!(parts(&log), None);
    }

    #[test]
    fn a_parts_progress_is_its_own() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("log");
        let names = ["Writing the area's terrain to the NAS", "Packing the new raw tiles onto the NAS"];
        let line = |i: usize| format!("parts: {i} {}\n", serde_json::to_string(&names).unwrap());
        // The part before's last word isn't this part's: none yet, then its own.
        std::fs::write(&log, format!("{}progress: 64/64 packs\n{}walking the cache\n", line(0), line(1))).unwrap();
        assert_eq!(progress(&log), None);
        let mut f = File::options().append(true).open(&log).unwrap();
        std::io::Write::write_all(&mut f, b"progress: 5/10 raw tiles\n").unwrap();
        assert_eq!(progress(&log), Some((5.0, 10.0, "raw tiles".to_string())));
    }

    #[test]
    fn exit_status() {
        let d = tempfile::tempdir().unwrap();
        let mut r = Running::start(spec(&["/bin/sh", "-c", "exit 3"]), 1, &[], d.path().join("log"), &d.path().join("job.json")).unwrap();
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
        let mut r = Running::start(spec(&["/bin/sh", "-c", "/bin/sleep 60 & echo started"]), 1, &[], d.path().join("log"), &rec).unwrap();
        while r.poll().unwrap().is_none() {
            std::thread::sleep(Duration::from_millis(50));
        }
        let pgid = r.pgid;
        assert!(crate::sys::signal_group(pgid, Signal::Probe), "the sleep is still in the group");
        let t = Instant::now();
        stop_orphan(&rec);
        assert!(t.elapsed() < Duration::from_secs(10), "stopped by SIGTERM, not after the grace");
        assert!(!crate::sys::signal_group(pgid, Signal::Probe));
        assert!(!rec.exists());
    }
}
