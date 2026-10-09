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
    /// Its progress as it last said (`said`), and the part it said it in: kept, as its parts are,
    /// while output since pushes the line out of the log's end.
    pub said: Option<(f64, f64, String, Option<usize>)>,
    /// When its progress last moved on: (the fraction, seconds since the epoch). A job whose
    /// progress hasn't moved in a long while may be stuck.
    pub moved: Option<(f64, u64)>,
    /// The worker threads it was given (RAYON_NUM_THREADS).
    pub threads: usize,
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
    /// Its step's targets and keys, and where it notes those it finished (crate::control::done):
    /// what an agent started after a crash records of it.
    #[serde(default)]
    work: Option<super::build::Work>,
    #[serde(default)]
    done_file: Option<PathBuf>,
}

use crate::sys::{group_members, process_start};

impl Running {
    /// Starts `spec` with `threads` worker threads (RAYON_NUM_THREADS) and `env`, its output
    /// appended to `log`, and records it in `record` (with `done_file`, where it notes the targets
    /// it finishes).
    pub fn start(spec: JobSpec, threads: usize, env: &[(&str, &str)], log: PathBuf, record: &Path, done_file: Option<&Path>) -> Result<Running> {
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
        let rec = Record { id: spec.id.clone(), pgid, started, leader_start: process_start(pgid).unwrap_or(0), work: spec.record.clone(), done_file: done_file.map(Path::to_path_buf) };
        std::fs::write(record, serde_json::to_vec(&rec)?)?;
        Ok(Running { spec, child, caffeinate, pgid, started, log, paused: None, pausing: None, started_at: Instant::now(), progress_base: None, parts: None, said: None, moved: None, threads })
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

/// What `stop_orphan` found of a job an earlier agent left: its step's targets and keys and where
/// it noted those it finished (when it said), and its process group while it can't be shown
/// stopped (some of it outlived the kill): work still running here, until the group is gone.
#[derive(Default)]
pub struct Orphan {
    pub done: Option<(super::build::Work, PathBuf)>,
    pub left: Option<Group>,
}

/// A job's process group as its record has it: its id, its leader's start time and the job's own,
/// and the job's id.
#[derive(Clone, Debug)]
pub struct Group {
    pub pgid: i32,
    pub leader_start: u64,
    pub started: u64,
    pub id: String,
}

impl Group {
    /// Whether the group is still the job's: its leader the process the job started; or, the
    /// leader gone, every member started after the job did (a group id isn't reused while any
    /// member lives, and is once none does).
    pub fn is_the_jobs(&self) -> bool {
        let members = if self.pgid > 1 { group_members(self.pgid) } else { Vec::new() };
        !members.is_empty()
            && match process_start(self.pgid) {
                Some(t) => self.leader_start != 0 && t == self.leader_start,
                None => members.iter().all(|&p| process_start(p).is_some_and(|t| t + 2 >= self.started)),
            }
    }
}

/// Stops a job left running by an agent that ended without stopping it (a crash, a kill), from its
/// record; removes the record. What it finished, for the agent to record (crate::control::done),
/// and whether it's gone (`Orphan`).
pub fn stop_orphan(record: &Path) -> Orphan {
    stop_orphan_within(record, Duration::from_secs(30))
}

/// `stop_orphan`, a SIGTERM given `grace` before the SIGKILL.
fn stop_orphan_within(record: &Path, grace: Duration) -> Orphan {
    let Ok(b) = std::fs::read(record) else { return Orphan::default() };
    let mut found = Orphan::default();
    if let Ok(r) = serde_json::from_slice::<Record>(&b) {
        found.done = r.work.clone().zip(r.done_file.clone());
        let g = Group { pgid: r.pgid, leader_start: r.leader_start, started: r.started, id: r.id };
        if g.is_the_jobs() {
            eprintln!("agent: stopping {} left running by an earlier agent (group {})", g.id, g.pgid);
            stop_group(g.pgid, grace, || {});
            found.left = g.is_the_jobs().then_some(g);
        }
    }
    std::fs::remove_file(record).ok();
    found
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
    end_of_run(log).0
}

/// A log's last 64 KB from its last run's start on (`end_of`), and whether that start is in it
/// (else the run said more than that since it began: what came before is out of sight).
fn end_of_run(log: &Path) -> (String, bool) {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = File::open(log) else { return (String::new(), true) };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let from = len.saturating_sub(64 << 10);
    let _ = f.seek(SeekFrom::Start(from));
    let mut b = Vec::new();
    let _ = f.read_to_end(&mut b);
    let s = String::from_utf8_lossy(&b).into_owned();
    match s.rfind(&format!("\n{RUN_START}")).map(|i| i + 1).or(s.starts_with(RUN_START).then_some(0)) {
        Some(i) => (s[i..].split_once('\n').map_or(String::new(), |(_, rest)| rest.to_string()), true),
        // (The whole log, when it's shorter than that: a log of no run's start is all one run's.)
        None => (s, from == 0),
    }
}

/// Says how far a job is, for the agent (`progress`): `progress: <done>/<total> <unit>` on stderr,
/// which goes to the job's log. The unit stays the same for the job's (or its part's) whole
/// course: the time left is measured from the pace in it (a word in brackets after it, as in
/// `steps (what's being done)`, may change).
pub fn report(done: u64, total: u64, unit: &str) {
    eprintln!("progress: {}/{total} {unit}", done.min(total));
}

/// Says how far a job is, `report`'s way, counting the item under way by how much of it is done
/// (`progress: 2.375/6 areas`): a job whose items each take long moves on within each.
pub fn report_f(done: f64, total: u64, unit: &str) {
    let d = (done.clamp(0.0, total as f64) * 1000.0).floor() / 1000.0;
    eprintln!("progress: {d}/{total} {unit}");
}

/// The stage a job said it began last (`stage`): a program's progress within it counts as the
/// stage's (`within`).
static FRAME: std::sync::Mutex<Option<(u64, u64, String)>> = std::sync::Mutex::new(None);

/// Says a job begins stage `k` of its `n` (`report`, in `unit`), and that a program run within it
/// says how far the stage is (`within`).
pub fn stage(k: u64, n: u64, unit: &str) {
    if let Ok(mut f) = FRAME.lock() {
        *f = Some((k, n, unit.to_string()));
    }
    report(k, n, unit);
}

/// The stage under way (`stage`) is `frac` done, as a program it runs says: `<k + frac>/<n>`.
pub fn within(frac: f64) {
    let frame = FRAME.lock().ok().and_then(|f| f.clone());
    if let Some((k, n, unit)) = frame {
        report_f(k as f64 + frac.clamp(0.0, 1.0), n, &unit);
    }
}

/// What a unit of progress is, for measuring its pace: the unit without a word in brackets after it
/// (`steps (clipping…)` and `steps (filtering…)` are both steps).
pub fn unit_key(unit: &str) -> &str {
    match unit.find(" (") {
        Some(i) if unit.ends_with(')') => &unit[..i],
        _ => unit,
    }
}

/// How far a line of a program's output says it is (0–1): a `progress: <done>/<total> …` line, or
/// a bar ending in a percentage (osmium's `[=====>        ]  52% `).
pub fn fraction_of(line: &str) -> Option<f64> {
    let l = line.trim();
    if let Some(rest) = l.strip_prefix("progress: ") {
        let (d, t) = rest.split_once(' ').map_or(rest, |x| x.0).split_once('/')?;
        let (d, t) = (d.parse::<f64>().ok()?, t.parse::<f64>().ok()?);
        return (t > 0.0).then(|| (d / t).clamp(0.0, 1.0));
    }
    if l.starts_with('[') {
        let pct: f64 = l.rsplit(']').next()?.trim().strip_suffix('%')?.trim().parse().ok()?;
        return Some((pct / 100.0).clamp(0.0, 1.0));
    }
    None
}

/// A program's output read as it comes, a line at a time to `each` (a bar's redraws, after carriage
/// returns, each a line, as each comes: osmium redraws its bar without a newline), until it closes.
pub fn each_line(mut from: impl std::io::Read, mut each: impl FnMut(&str)) {
    let mut buf = [0u8; 8192];
    let mut line: Vec<u8> = Vec::new();
    let mut say = |line: &mut Vec<u8>| {
        if !line.iter().all(u8::is_ascii_whitespace) {
            each(&String::from_utf8_lossy(line));
        }
        line.clear();
    };
    loop {
        let k = match from.read(&mut buf) {
            Ok(0) => break,
            Ok(k) => k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        for &b in &buf[..k] {
            if b == b'\n' || b == b'\r' {
                say(&mut line);
            } else {
                line.push(b);
            }
        }
    }
    say(&mut line);
}

/// Runs `c` with its standard error read here as it comes: each fraction it says (`fraction_of`)
/// to `seen`, every other line on to this job's log. (osmium says how far it is with `--progress`.)
pub fn run_watched(c: &mut Command, mut seen: impl FnMut(f64)) -> std::io::Result<ExitStatus> {
    let mut child = c.stderr(Stdio::piped()).spawn()?;
    if let Some(err) = child.stderr.take() {
        each_line(err, |l| match fraction_of(l) {
            Some(f) => seen(f),
            None => eprintln!("{l}"),
        });
    }
    child.wait()
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

/// What a job's log says of how far it is (`said`).
#[derive(Clone, Debug, PartialEq)]
pub enum Said {
    /// Its last `progress: <done>/<total> <unit>` line.
    Progress(f64, f64, String),
    /// Nothing since the part under way, or this run, began.
    NoneYet,
    /// Neither within the log's end: output since pushed it out of sight, and what it last said
    /// holds.
    Unknown,
}

/// How far a job says it is: its log's last `progress: <done>/<total> <unit>` line (build steps
/// print them: scenic-build's `progress`).
pub fn said(log: &Path) -> Said {
    let (s, whole) = end_of_run(log);
    for l in s.lines().rev().map(|l| l.rsplit('\r').next().unwrap_or(l).trim()) {
        // (One said before the part under way began, or this run, is another's: none yet.)
        if l.starts_with("parts: ") || l.starts_with("=== run of ") {
            return Said::NoneYet;
        }
        let Some(rest) = l.strip_prefix("progress: ") else { continue };
        let (frac, unit) = rest.split_once(' ').unwrap_or((rest, ""));
        let Some((d, t)) = frac.split_once('/') else { continue };
        let (Ok(d), Ok(t)) = (d.parse::<f64>(), t.parse::<f64>()) else { continue };
        if t > 0.0 {
            return Said::Progress(d.min(t), t, unit.trim().to_string());
        }
    }
    if whole {
        Said::NoneYet
    } else {
        Said::Unknown
    }
}

/// How far a job says it is (`said`), when its log's end says.
pub fn progress(log: &Path) -> Option<(f64, f64, String)> {
    match said(log) {
        Said::Progress(d, t, u) => Some((d, t, u)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(cmd: &[&str]) -> JobSpec {
        JobSpec { id: "t".into(), what: "test".into(), cmd: cmd.iter().map(|s| s.to_string()).collect(), needs: Needs::default(), restart_after_sleep: false, record: None }
    }

    /// What `f` finds of job `r`, once it does: the tests wait for what the job did, not for a
    /// time. A job runs at utility priority, and on a loaded Mac (other builds beside it) its shell
    /// can take seconds to start or to take its next step. Five minutes is a watchdog: then the job
    /// is stopped, and the test fails.
    fn wait_for<T>(r: &mut Running, what: &str, mut f: impl FnMut(&mut Running) -> Option<T>) -> T {
        let t = Instant::now();
        loop {
            if let Some(x) = f(r) {
                return x;
            }
            if t.elapsed() > Duration::from_secs(300) {
                r.stop(Duration::from_secs(1));
                panic!("{what}: not within five minutes");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn runs_pauses_and_stops() {
        let d = tempfile::tempdir().unwrap();
        let rec = d.path().join("job.json");
        let mut r = Running::start(spec(&["/bin/sh", "-c", "echo hello; sleep 3600"]), 2, &[], d.path().join("log"), &rec, None).unwrap();
        assert!(rec.exists());
        // Running: what it says reaches its log. Paused, it hasn't ended; stopped, it has, by the
        // stop's signal.
        wait_for(&mut r, "its hello in its log", |r| tail(&r.log, 5).contains("hello").then_some(()));
        r.pause("test");
        assert!(r.poll().unwrap().is_none());
        r.resume();
        r.stop(Duration::from_secs(5));
        assert!(r.poll().unwrap().is_some_and(|st| st.code().is_none()), "stopped by a signal");
        assert!(tail(&r.log, 5).contains("hello"));
    }

    #[test]
    fn a_job_asked_to_stop_does_at_its_next_safe_point() {
        // A job that builds its targets in turn, noting each done, and stops at the next safe point
        // once its channel says so (crate::control's protocol, as scenic-build's steps). It says
        // which target it's on; its third takes until the ask comes (a minute at most), so the ask
        // comes while it builds that one, whatever the Mac's load: it finishes that one, notes it
        // done, and ends there, starting no other.
        let d = tempfile::tempdir().unwrap();
        let (control, done, on) = (d.path().join("control"), d.path().join("done.txt"), d.path().join("on"));
        std::fs::write(&control, b"run").unwrap();
        let script = r#"for t in 6/1/1 6/1/2 6/1/3 6/1/4 6/1/5 6/1/6 6/1/7 6/1/8; do
            if grep -q drain "$SCENIC_CONTROL"; then exit 75; fi
            echo "$t" > "$ON"
            if [ "$t" = 6/1/3 ]; then
                i=0; until grep -q drain "$SCENIC_CONTROL"; do i=$((i + 1)); [ "$i" -le 6000 ] || exit 1; sleep 0.01; done
            fi
            echo "unit $t" >> "$SCENIC_DONE"
        done"#;
        let env = [(crate::control::CONTROL_ENV, control.to_str().unwrap()), (crate::control::DONE_ENV, done.to_str().unwrap()), ("ON", on.to_str().unwrap())];
        let mut r = Running::start(spec(&["/bin/sh", "-c", script]), 1, &env, d.path().join("log"), &d.path().join("job.json"), Some(&done)).unwrap();
        wait_for(&mut r, "on its third target", |_| std::fs::read_to_string(&on).is_ok_and(|s| s.trim() == "6/1/3").then_some(()));
        std::fs::write(&control, b"drain").unwrap();
        let st = wait_for(&mut r, "stopped at its next safe point", |r| r.poll().unwrap());
        assert_eq!(st.code(), Some(crate::control::PAUSED_EXIT));
        assert_eq!(crate::control::read_done(&done, "unit"), ["6/1/1", "6/1/2", "6/1/3"]);
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
    fn progress_said_long_ago_is_out_of_sight_not_gone() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("log");
        // Said, then 100 KB of a program's output: out of the log's end, so not known.
        let noise = "x".repeat(99) + "\n";
        std::fs::write(&log, format!("{RUN_START}t (1) ===\nprogress: 2.5/6 areas\n{}", noise.repeat(1000))).unwrap();
        assert_eq!(said(&log), Said::Unknown);
        // Short: all of it this run's, and nothing said yet.
        std::fs::write(&log, format!("{RUN_START}t (1) ===\nstarting\n")).unwrap();
        assert_eq!(said(&log), Said::NoneYet);
        std::fs::write(&log, "starting\nprogress: 2.375/6 areas\n").unwrap();
        assert_eq!(said(&log), Said::Progress(2.375, 6.0, "areas".into()));
    }

    #[test]
    fn fractions_and_bars() {
        assert_eq!(fraction_of("progress: 3/4 vertices"), Some(0.75));
        assert_eq!(fraction_of("[=========>                    ]  30% "), Some(0.3));
        assert_eq!(fraction_of("[======] 100%"), Some(1.0));
        assert_eq!(fraction_of("progress: 1/0 none"), None);
        assert_eq!(fraction_of("cache: reused 5 of 6"), None);
        assert_eq!(unit_key("steps (clipping the areas)"), "steps");
        assert_eq!(unit_key("areas"), "areas");
        assert_eq!(unit_key("MB (of 3)x"), "MB (of 3)x");
        let mut lines = Vec::new();
        each_line(&b"[>   ]  0% \r[=>  ]  50% \rdone\nlast"[..], |l| lines.push(l.to_string()));
        assert_eq!(lines, ["[>   ]  0% ", "[=>  ]  50% ", "done", "last"]);
        // A program's progress, seen as it runs; its other lines on to the log.
        let mut seen = Vec::new();
        let st = run_watched(Command::new("/bin/sh").args(["-c", "echo 'progress: 1/4 x' >&2; echo hi >&2; printf '[=> ] 75%%\r' >&2"]), |f| seen.push(f)).unwrap();
        assert!(st.success());
        assert_eq!(seen, [0.25, 0.75]);
    }

    #[test]
    fn a_bar_redrawn_without_a_newline_is_seen_as_it_comes() {
        // (osmium's: carriage returns alone, the newline only at its end.) The program goes on past
        // its first bar once that bar's been seen here: seen only at its end, it would wait for it
        // in vain, give up after a minute and fail.
        let d = tempfile::tempdir().unwrap();
        let mark = d.path().join("seen");
        let script = r#"printf '[=>  ]  50%% \r' >&2
            i=0; until [ -e "$SEEN" ]; do i=$((i + 1)); [ "$i" -le 6000 ] || exit 1; sleep 0.01; done
            printf '[===]  100%% \n' >&2"#;
        let mut seen = Vec::new();
        let st = run_watched(Command::new("/bin/sh").args(["-c", script]).env("SEEN", &mark), |f| {
            if f == 0.5 {
                std::fs::write(&mark, b"").unwrap();
            }
            seen.push(f);
        })
        .unwrap();
        assert!(st.success(), "its first bar seen only as it ended");
        assert_eq!(seen, [0.5, 1.0]);
    }

    #[test]
    fn exit_status() {
        let d = tempfile::tempdir().unwrap();
        let mut r = Running::start(spec(&["/bin/sh", "-c", "exit 3"]), 1, &[], d.path().join("log"), &d.path().join("job.json"), None).unwrap();
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
        let mut r = Running::start(spec(&["/bin/sh", "-c", "/bin/sleep 3600 & echo started"]), 1, &[], d.path().join("log"), &rec, None).unwrap();
        while r.poll().unwrap().is_none() {
            std::thread::sleep(Duration::from_millis(50));
        }
        let pgid = r.pgid;
        assert!(crate::sys::signal_group(pgid, Signal::Probe), "the sleep is still in the group");
        // (Given ten minutes' grace, it's stopped well before: by the SIGTERM, which a sleep
        // doesn't outlive; one that didn't stop it would have it wait the grace out.)
        let t = Instant::now();
        stop_orphan_within(&rec, Duration::from_secs(600));
        assert!(t.elapsed() < Duration::from_secs(600), "stopped by SIGTERM, not after the grace");
        assert!(!crate::sys::signal_group(pgid, Signal::Probe));
        assert!(!rec.exists());
    }
}
