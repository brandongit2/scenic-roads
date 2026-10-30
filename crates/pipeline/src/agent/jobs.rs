//! Running one build job (docs/macs.md and docs/building.md §2): a child process in its own process group, at
//! utility priority (`taskpolicy -c utility`), the Mac kept awake while it runs (`caffeinate -i -s
//! -w`: no idle sleep, on battery too, and no sleep on mains power) but not while it's paused
//! (`SIGSTOP` to the group while a condition it needs lapses; `SIGCONT` when it holds again), and
//! stopped as a group.
//!
//! A job outlives its agent (docs/building.md §2, Restarts): its record on disk names all an agent
//! needs to tend it (its spec, threads, log, and the slot's lease as the agent last saved it,
//! `save_hand`), and a shell at the head of its group writes its exit status to a file beside the
//! record (`status_path`), so the agent after it, not its parent, still learns how it ended. An
//! agent that restarts (a newer app, its launcher restarted, a crash) leaves its jobs running, and
//! the next one takes them over (`adopt`).

use crate::sys::Signal;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The NAS project folder as a child process's `--root` argument (its spec's): the spawners' one
/// way out of the type (tests/nas_access.rs).
pub fn root_arg(root: &store::nasfs::NasRoot) -> String {
    root.unchecked_path().to_string_lossy().into_owned()
}

/// What a job needs to run: the NAS (every job's: a test's may not). It runs at any charge, at home
/// or away (the owner's choice: docs/pool.md §7.7); a record an older app wrote, with its power and
/// home, reads as this.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Needs {
    #[serde(default)]
    pub nas: bool,
}

/// What a running job is doing now (docs/pool.md §10, Live; task #518), as the agent reads it for
/// its status and its lease's beat: the phase its main thread has open (crate::timings, said in its
/// counts file: store::nasio::set_phase) and since when; the target under way (its
/// costs file: crate::agent::memguard::targets) and since when, and how many of its targets ended;
/// its NAS bytes read and written (its processes' counts, store::nasio) and its internet bytes in
/// and out (the proxy's count of its tag, crate::budget::net), since it began. Each None: not said
/// (a step that times no phases or notes no costs, a job its agent gave no folder or tag).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Live {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// Unix s.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_since: Option<u64>,
    /// Its cost key (crate::coord::cost_key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Unix s.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_since: Option<u64>,
    #[serde(default)]
    pub targets_done: usize,
    /// Its targets (0: a job of none of its own).
    #[serde(default)]
    pub targets: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nas_read: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nas_wrote: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net_in: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net_out: Option<u64>,
}

impl Live {
    /// A job's `Live` from its counts folder (store::nasio::ENV), its costs file, its targets' count
    /// and its internet bytes (in, out).
    pub fn read(counts: &Path, costs: &Path, targets: usize, net: Option<(u64, u64)>) -> Live {
        let phase = store::nasio::phase_in(counts).filter(|p| !p.0.is_empty());
        // (An older process says its phase's class after a tab: left out.)
        let name = phase.as_ref().map(|(t, _)| t.split_once('\t').map_or(t.as_str(), |(n, _)| n).to_string());
        let t = super::memguard::targets(costs);
        let nas = store::nasio::sum_dir(counts);
        let at = |c: store::nasio::Count| nas.map(|n| n[c as usize]);
        Live {
            phase: name,
            phase_since: phase.map(|p| p.1 / 1000),
            target_since: t.on.as_ref().map(|o| o.1),
            target: t.on.map(|o| o.0),
            targets_done: t.ended,
            targets,
            nas_read: at(store::nasio::Count::ReadBytes),
            nas_wrote: at(store::nasio::Count::WroteBytes),
            net_in: net.map(|n| n.0),
            net_out: net.map(|n| n.1),
        }
    }
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
    /// Its group's head, this process's child; None when an agent before this one started it
    /// (`adopt`): its end is then read from its status file (`status`).
    child: Option<Child>,
    /// Where its group's head writes its exit status as it ends (`status_path`).
    status: PathBuf,
    /// An agent before this one started it (`adopt`).
    pub adopted: bool,
    /// An adopted job's group ended without a status (killed while no agent tended it).
    pub vanished: bool,
    caffeinate: Option<store::spawn::Detached>,
    pub pgid: i32,
    /// Its group's head's start time (`process_start`, 0 when unknown): with `started`, what tells
    /// its group from another given the same id once it's gone (`group`, `Group::is_the_jobs`).
    leader_start: u64,
    pub started: u64,
    pub log: PathBuf,
    /// Why it's frozen where it is, while it is.
    pub paused: Option<String>,
    /// Why it's stopping at its next safe point (crate::control), while it is.
    pub pausing: Option<String>,
    /// When it started (one taken over: when the agent before this one started it).
    pub(crate) started_at: Instant,
    /// Where its pace is measured from (time, fraction, unit, part): where this kind of progress was
    /// first seen; a taken-over job's first part from its start (agent::pace_base).
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
    /// What an agent after this one needs to take it over (`adopt`): its spec, threads and log
    /// (none in a record an older app wrote: such a job is stopped, `stop_orphan`).
    #[serde(default)]
    spec: Option<JobSpec>,
    #[serde(default)]
    threads: usize,
    #[serde(default)]
    log: Option<PathBuf>,
    /// Why it was frozen when the record was last saved (`save_hand`).
    #[serde(default)]
    paused: Option<String>,
    /// Its agent's slot as last saved (the lease, its grant): the agent's to read (`save_hand`).
    #[serde(default)]
    hand: Option<serde_json::Value>,
}

/// Where the job of record `record` has its exit status written as it ends: beside it
/// (`job.json`: `job.status`).
pub fn status_path(record: &Path) -> PathBuf {
    record.with_extension("status")
}

/// The shell at the head of a job's group: runs the job's command (its arguments after the status
/// file's), then writes its exit status to that file (whole: by a temporary name, renamed), and
/// ends as the command did (a signal's end raised again, so its parent sees it as before).
const HEAD: &str = r#"s=$1; shift; "$@"; c=$?; printf '%s\n' "$c" > "$s.tmp" && mv -f "$s.tmp" "$s"; if [ "$c" -gt 128 ]; then kill -"$((c - 128))" $$; fi; exit "$c""#;

use crate::sys::{group_members, process_start};

impl Running {
    /// Starts `spec` with `threads` worker threads (RAYON_NUM_THREADS) and `env`, its output
    /// appended to `log`, and records it in `record` (with `done_file`, where it notes the targets
    /// it finishes); under `sandbox`, a profile of `sandbox-exec` (crate::budget::SANDBOX: no way
    /// out but through its agent's proxy), when given.
    pub fn start(
        spec: JobSpec,
        threads: usize,
        env: &[(&str, &str)],
        log: PathBuf,
        record: &Path,
        done_file: Option<&Path>,
        sandbox: Option<&str>,
    ) -> Result<Running> {
        if let Some(d) = log.parent() {
            std::fs::create_dir_all(d)?;
        }
        let mut out =
            File::options().create(true).append(true).open(&log).with_context(|| format!("open {}", log.display()))?;
        // (A job's log keeps its earlier runs': this run's begins here, and only what follows is
        // its progress, parts and last lines.)
        std::io::Write::write_all(&mut out, format!("{RUN_START}{} ({}) ===\n", spec.id, now_s()).as_bytes())?;
        let err = out.try_clone()?;
        // (In the tests, every job's command as the agent made it, checked against its program's
        // table: one the program would refuse fails the test.)
        #[cfg(test)]
        if let Some(Err(e)) = crate::cli::check_command(&spec.cmd) {
            panic!("{e}");
        }
        let (prog, args) = spec.cmd.split_first().context("empty command")?;
        // (Its status written by the shell at its group's head, `HEAD`: read by an agent that
        // adopts it, the parent then not this process.)
        let status = status_path(record);
        std::fs::remove_file(&status).ok();
        let mut c = store::spawn::command("/bin/sh");
        c.args(["-c", HEAD, "sh"]).arg(&status);
        c.args(["/usr/sbin/taskpolicy", "-c", "utility"]);
        if let Some(profile) = sandbox {
            c.args(["/usr/bin/sandbox-exec", "-p", profile]);
        }
        c.arg(prog).args(args);
        // (The Python steps' output reaches the log, and so the status, as they print it: piped, it
        // would wait in their buffers, for minutes.)
        c.env("RAYON_NUM_THREADS", threads.to_string())
            .env("PYTHONUNBUFFERED", "1")
            .envs(env.iter().copied())
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err);
        // Its own process group, so pausing and stopping reach every process it starts.
        crate::sys::own_group(&mut c);
        let child = store::spawn::spawn(&mut c).with_context(|| format!("start {}", spec.id))?;
        let pgid = child.id() as i32;
        #[cfg(test)]
        test_groups::track(pgid);
        let caffeinate = keep_awake(child.id());
        let started = now_s();
        let leader_start = process_start(pgid).unwrap_or(0);
        let rec = Record {
            id: spec.id.clone(),
            pgid,
            started,
            leader_start,
            work: spec.record.clone(),
            done_file: done_file.map(Path::to_path_buf),
            spec: Some(spec.clone()),
            threads,
            log: Some(log.clone()),
            paused: None,
            hand: None,
        };
        std::fs::write(record, serde_json::to_vec(&rec)?)?;
        Ok(Running {
            spec,
            child: Some(child),
            status,
            adopted: false,
            vanished: false,
            caffeinate,
            pgid,
            leader_start,
            started,
            log,
            paused: None,
            pausing: None,
            started_at: Instant::now(),
            progress_base: None,
            parts: None,
            said: None,
            moved: None,
            threads,
        })
    }

    /// Pauses the job's whole process group (`why` goes to the status), and lets the Mac sleep.
    pub fn pause(&mut self, why: &str) {
        if self.paused.is_none() {
            crate::sys::signal_group(self.pgid, Signal::Stop);
            if let Some(mut c) = self.caffeinate.take() {
                let _ = c.kill();
            }
        }
        self.paused = Some(why.to_string());
    }

    pub fn resume(&mut self) {
        if self.paused.take().is_some() {
            crate::sys::signal_group(self.pgid, Signal::Cont);
            self.caffeinate = keep_awake(self.pgid as u32);
        }
    }

    /// Its exit status once it has finished: its child's; an adopted job's from its status file,
    /// or, its group gone without one (killed while no agent tended it: `vanished`), as killed.
    pub fn poll(&mut self) -> Result<Option<ExitStatus>> {
        if let Some(c) = self.child.as_mut() {
            return Ok(c.try_wait()?);
        }
        if let Some(st) = read_status(&self.status) {
            return Ok(Some(st));
        }
        if !group_members(self.pgid).is_empty() {
            return Ok(None);
        }
        // (Its status may have been written as the group emptied.)
        Ok(Some(read_status(&self.status).unwrap_or_else(|| {
            self.vanished = true;
            killed()
        })))
    }

    /// Waits for it to finish (the tests': a job that ends by itself), its exit status.
    #[cfg(test)]
    pub fn wait(&mut self) -> Result<ExitStatus> {
        if let Some(c) = self.child.as_mut() {
            return Ok(c.wait()?);
        }
        loop {
            if let Some(st) = self.poll()? {
                return Ok(st);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Stops the whole group: SIGTERM (after SIGCONT, so a paused job can handle it), then SIGKILL
    /// after `grace` to whatever of it is left.
    pub fn stop(&mut self, grace: Duration) {
        let child = &mut self.child;
        stop_group(self.pgid, grace, || {
            if let Some(c) = child.as_mut() {
                let _ = c.try_wait();
            }
        });
        if let Some(c) = self.child.as_mut() {
            let _ = c.wait();
        }
        if let Some(c) = self.caffeinate.as_mut() {
            let _ = c.kill();
        }
    }

    /// Left running for the agent after this one (`adopt`): its record kept with `hand`, the
    /// agent's slot (`save_hand`); this agent's hold on the Mac's sleep let go (the next keeps it
    /// awake again).
    pub fn hand_over(mut self, record: &Path, hand: serde_json::Value) {
        save_hand(record, &self, hand);
        if let Some(mut c) = self.caffeinate.take() {
            let _ = c.kill();
        }
        // (Its child dropped unwaited: it goes on, reaped by launchd once this process has ended.)
        self.child.take();
    }

    /// Its process group as signals must know it (`Group::is_the_jobs` before each, by a thread
    /// that doesn't hold its child: crate::agent::slots).
    pub fn group(&self) -> Group {
        Group { pgid: self.pgid, leader_start: self.leader_start, started: self.started, id: self.spec.id.clone() }
    }

    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }
}

/// Keeps the Mac awake while process `pid` runs: no idle sleep (on battery too: -i), no system sleep
/// on mains power (-s), ending with it (-w).
/// Started `detached`: it lives as long as the job, so holds nothing of this process's.
fn keep_awake(pid: u32) -> Option<store::spawn::Detached> {
    store::spawn::detached("/usr/bin/caffeinate", &["-i", "-s", "-w", &pid.to_string()]).ok()
}

/// An exit status read from a job's status file (`HEAD`), when there's one that reads: a code past
/// 128 as the signal it says.
fn read_status(p: &Path) -> Option<ExitStatus> {
    let code: i32 = std::fs::read_to_string(p).ok()?.trim().parse().ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        Some(if code > 128 { ExitStatus::from_raw(code - 128) } else { ExitStatus::from_raw((code & 0xff) << 8) })
    }
    #[cfg(not(unix))]
    {
        let _ = code;
        None
    }
}

/// The status of a job killed (SIGKILL) with no word of its end.
fn killed() -> ExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        ExitStatus::from_raw(9)
    }
    #[cfg(not(unix))]
    {
        unreachable!("jobs run on macOS")
    }
}

/// Saves slot state `hand` (the agent's: its lease, its grant) and whether `r` is frozen into its
/// record, for an agent after this one to take it over (`adopt`). Written whole.
pub fn save_hand(record: &Path, r: &Running, hand: serde_json::Value) {
    let Ok(b) = std::fs::read(record) else { return };
    let Ok(mut rec) = serde_json::from_slice::<Record>(&b) else { return };
    rec.hand = Some(hand);
    rec.paused = r.paused.clone();
    if let Ok(b) = serde_json::to_vec(&rec) {
        if let Err(e) = store::whole::write(record, &b) {
            eprintln!("agent: saving {}'s record: {e:#}", r.spec.id);
        }
    }
}

/// The slot state saved with the record `record` (`save_hand`), if any.
pub fn saved_hand(record: &Path) -> Option<serde_json::Value> {
    serde_json::from_slice::<Record>(&std::fs::read(record).ok()?).ok()?.hand
}

/// A job an agent before this one left running, taken over from its record `record`: its group
/// still the job's, or ended with its status written (its end then seen at the agent's next look);
/// with the slot state that agent saved (`save_hand`). None when the record names no such job (none
/// there, one an older app wrote, no slot state saved), its group gone without a status, or not the
/// job's: `stop_orphan` then. The record stays.
pub fn adopt(record: &Path) -> Option<(Running, serde_json::Value)> {
    let rec: Record = serde_json::from_slice(&std::fs::read(record).ok()?).ok()?;
    let (spec, hand, log) = (rec.spec?, rec.hand?, rec.log?);
    let status = status_path(record);
    let g = Group { pgid: rec.pgid, leader_start: rec.leader_start, started: rec.started, id: rec.id };
    // (Its group gone, its status written: ended already. Its group id may since be another's:
    // none kept (0), so nothing signals it, nothing keeps the Mac awake by it nor watches it.)
    let ended = !g.is_the_jobs();
    if ended && read_status(&status).is_none() {
        return None;
    }
    let pgid = if ended { 0 } else { rec.pgid };
    let ran = Duration::from_secs(now_s().saturating_sub(rec.started));
    let caffeinate = if rec.paused.is_none() && !ended { keep_awake(pgid as u32) } else { None };
    Some((
        Running {
            spec,
            child: None,
            status,
            adopted: true,
            vanished: false,
            caffeinate,
            pgid,
            leader_start: rec.leader_start,
            started: rec.started,
            log,
            paused: rec.paused,
            pausing: None,
            started_at: Instant::now().checked_sub(ran).unwrap_or_else(Instant::now),
            progress_base: None,
            parts: None,
            said: None,
            moved: None,
            threads: rec.threads.max(1),
        },
        hand,
    ))
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
    let lines: Vec<&str> = s
        .lines()
        .map(|l| l.rsplit('\r').next().unwrap_or(l))
        .filter(|l| !l.trim().is_empty() && !l.starts_with("parts: "))
        .collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// What a failed run said of its failure: its error (anyhow's `Error: …` and its causes) or its
/// panic, from its last run's log; empty when it said neither (killed, say). Found by its marker,
/// not by position: the timings' table and progress lines may follow it.
pub fn error_of(log: &Path) -> String {
    let s = end_of(log);
    let lines: Vec<&str> = s.lines().map(|l| l.rsplit('\r').next().unwrap_or(l)).collect();
    let Some(i) = lines.iter().rposition(|l| l.starts_with("Error: ") || l.contains("panicked at")) else {
        return String::new();
    };
    let panic = lines[i].contains("panicked at");
    let mut out = vec![lines[i].trim_end()];
    for l in &lines[i + 1..] {
        // anyhow's causes ("Caused by:", then indented lines); a panic's message on its next line.
        let more =
            l.starts_with("Caused by:") || l.starts_with("    ") || l.trim().is_empty() || (panic && out.len() == 1);
        if !more || out.len() >= 12 {
            break;
        }
        if !l.trim().is_empty() {
            out.push(l.trim_end());
        }
    }
    out.join("\n")
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
/// (`progress: 2.375/6 z6 tiles`): a job whose items each take long moves on within each.
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
    let mut child = store::spawn::spawn(&mut c.stderr(Stdio::piped()))?;
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

/// Test builds: every job group a test starts, killed (by its exact group id) when the test
/// process exits, however its test ended (#654: a panicking test left its job running 16.5 h).
/// Not run if the process is killed outright; the stand-in programs also end when their temp
/// folder is gone. A group still alive then is reported on stderr.
#[cfg(test)]
mod test_groups {
    use std::sync::Mutex;

    static STARTED: Mutex<Vec<i32>> = Mutex::new(Vec::new());

    extern "C" fn reap() {
        for &g in STARTED.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            if !crate::sys::group_members(g).is_empty() {
                eprintln!("test left job group {g} running: killed");
                crate::sys::signal_group(g, crate::sys::Signal::Kill);
            }
        }
    }

    pub fn track(pgid: i32) {
        let mut v = STARTED.lock().unwrap_or_else(|e| e.into_inner());
        if v.is_empty() {
            // SAFETY: registers a plain extern "C" fn with no captured state.
            unsafe { libc::atexit(reap) };
        }
        v.push(pgid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(cmd: &[&str]) -> JobSpec {
        JobSpec {
            id: "t".into(),
            what: "test".into(),
            cmd: cmd.iter().map(|s| s.to_string()).collect(),
            needs: Needs::default(),
            restart_after_sleep: false,
            record: None,
        }
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
            if t.elapsed() > store::testkit::WATCHDOG {
                r.stop(Duration::from_secs(1));
                panic!("{what}: not within {:?}", store::testkit::WATCHDOG);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn runs_pauses_and_stops() {
        let d = tempfile::tempdir().unwrap();
        let rec = d.path().join("job.json");
        let mut r = Running::start(
            spec(&["/bin/sh", "-c", "echo hello; sleep 3600"]),
            2,
            &[],
            d.path().join("log"),
            &rec,
            None,
            None,
        )
        .unwrap();
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
        let env = [
            (crate::control::CONTROL_ENV, control.to_str().unwrap()),
            (crate::control::DONE_ENV, done.to_str().unwrap()),
            ("ON", on.to_str().unwrap()),
        ];
        let mut r = Running::start(
            spec(&["/bin/sh", "-c", script]),
            1,
            &env,
            d.path().join("log"),
            &d.path().join("job.json"),
            Some(&done),
            None,
        )
        .unwrap();
        wait_for(&mut r, "on its third target", |_| {
            std::fs::read_to_string(&on).is_ok_and(|s| s.trim() == "6/1/3").then_some(())
        });
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
        std::fs::write(
            &log,
            format!("{}progress: 0/3 parts (Getting ready)\nosmium: done\n{}some output\n", line(0), line(1)),
        )
        .unwrap();
        assert_eq!(parts(&log), Some((1, names.iter().map(|s| s.to_string()).collect())));
        // A run after it: none of the earlier run's.
        let mut f = File::options().append(true).open(&log).unwrap();
        std::io::Write::write_all(&mut f, format!("{RUN_START}t (1) ===\nstarting\n").as_bytes()).unwrap();
        assert_eq!((parts(&log), progress(&log), tail(&log, 5).as_str()), (None, None, "starting"));
        std::fs::write(
            &log,
            format!("{}progress: 0/3 parts (Getting ready)\nosmium: done\n{}some output\n", line(0), line(1)),
        )
        .unwrap();
        // Not text to show; and a line out of range is none.
        assert_eq!(tail(&log, 5), "progress: 0/3 parts (Getting ready)\nosmium: done\nsome output");
        std::fs::write(&log, format!("parts: 7 {}\n", serde_json::to_string(&names).unwrap())).unwrap();
        assert_eq!(parts(&log), None);
    }

    /// Task #438: a failed job's error is found though the timings' table and a backtrace follow
    /// it (depth-lo's "Too many open files" was past the log's last 20 lines, and the agent's log
    /// said only "failed after N s"); a panic's message too; none when it said neither.
    #[test]
    fn a_failures_error_is_found_wherever_it_is() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("log");
        let timings = "timings: composing  14.5 s\n".repeat(30);
        std::fs::write(
            &log,
            format!("{RUN_START}t (1) ===\nprogress: 0.09/3 zooms\n{timings}Error: the depth source globathy\n\nCaused by:\n    0: /x/globathy-8-81-114.tif\n    1: Too many open files (os error 24)\n\nStack backtrace:\n   0: foo\n"),
        )
        .unwrap();
        assert_eq!(
            error_of(&log),
            "Error: the depth source globathy\nCaused by:\n    0: /x/globathy-8-81-114.tif\n    1: Too many open files (os error 24)"
        );
        std::fs::write(&log, "a\nthread 'main' panicked at src/x.rs:1:2:\nboom\nnote: run with RUST_BACKTRACE=1\n")
            .unwrap();
        assert_eq!(error_of(&log), "thread 'main' panicked at src/x.rs:1:2:\nboom");
        std::fs::write(&log, "progress: 1/2 z6 tiles\n").unwrap();
        assert_eq!(error_of(&log), "");
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
        std::fs::write(&log, format!("{RUN_START}t (1) ===\nprogress: 2.5/6 z6 tiles\n{}", noise.repeat(1000)))
            .unwrap();
        assert_eq!(said(&log), Said::Unknown);
        // Short: all of it this run's, and nothing said yet.
        std::fs::write(&log, format!("{RUN_START}t (1) ===\nstarting\n")).unwrap();
        assert_eq!(said(&log), Said::NoneYet);
        std::fs::write(&log, "starting\nprogress: 2.375/6 z6 tiles\n").unwrap();
        assert_eq!(said(&log), Said::Progress(2.375, 6.0, "z6 tiles".into()));
    }

    #[test]
    fn fractions_and_bars() {
        assert_eq!(fraction_of("progress: 3/4 vertices"), Some(0.75));
        assert_eq!(fraction_of("[=========>                    ]  30% "), Some(0.3));
        assert_eq!(fraction_of("[======] 100%"), Some(1.0));
        assert_eq!(fraction_of("progress: 1/0 none"), None);
        assert_eq!(fraction_of("cache: reused 5 of 6"), None);
        assert_eq!(unit_key("steps (clipping the tiles)"), "steps");
        assert_eq!(unit_key("z6 tiles"), "z6 tiles");
        assert_eq!(unit_key("MB (of 3)x"), "MB (of 3)x");
        let mut lines = Vec::new();
        each_line(&b"[>   ]  0% \r[=>  ]  50% \rdone\nlast"[..], |l| lines.push(l.to_string()));
        assert_eq!(lines, ["[>   ]  0% ", "[=>  ]  50% ", "done", "last"]);
        // A program's progress, seen as it runs; its other lines on to the log.
        let mut seen = Vec::new();
        let st = run_watched(
            store::spawn::command("/bin/sh")
                .args(["-c", "echo 'progress: 1/4 x' >&2; echo hi >&2; printf '[=> ] 75%%\r' >&2"]),
            |f| seen.push(f),
        )
        .unwrap();
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
        let st = run_watched(store::spawn::command("/bin/sh").args(["-c", script]).env("SEEN", &mark), |f| {
            if f == 0.5 {
                std::fs::write(&mark, b"").unwrap();
            }
            seen.push(f);
        })
        .unwrap();
        assert!(st.success(), "its first bar seen only as it ended");
        assert_eq!(seen, [0.5, 1.0]);
    }

    /// A job under the budgets' sandbox (crate::budget::SANDBOX) can't connect out but to this
    /// Mac: a step that goes around the proxy fails at once (denied, not timed out); through the
    /// proxy, its request is metered (a plain HTTP request to a Wikimedia host, answered here).
    #[test]
    fn a_sandboxed_job_reaches_out_only_through_the_proxy() {
        use crate::budget::{Budget, Proxy, StepClock};
        use std::sync::Arc;
        let d = tempfile::tempdir().unwrap();
        // (This Mac's own: a listener here, its backlog answering nc's connect; the upstream the
        // proxy is told every host is.)
        let here = store::spawn::listen("127.0.0.1:0").unwrap();
        let port = here.local_addr().unwrap().port();
        let up = store::spawn::listen("127.0.0.1:0").unwrap();
        let up_addr = up.local_addr().unwrap();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            up.set_nonblocking(false).ok();
            // (One request: its head read, an answer, closed.)
            #[allow(clippy::disallowed_methods)]
            let (mut s, _) = up.accept().unwrap();
            let mut b = [0u8; 4096];
            let n = s.read(&mut b).unwrap();
            let first = String::from_utf8_lossy(&b[..n]).lines().next().unwrap_or_default().to_string();
            write!(s, "HTTP/1.0 200 OK\r\nContent-Type: text/plain\r\n\r\nupstream saw: {first}\n").unwrap();
        });
        let budget = Arc::new(
            Budget::new(
                &store::nasfs::NasRoot::local_for_test(&d.path().join("nas")),
                "m-test",
                Arc::new(StepClock::at(1_800_000_000_000)),
            )
            .with_ledger(),
        );
        let proxy = Proxy::start(budget.clone(), Some(Arc::new(move |_: &str, _: u16| Some(up_addr)))).unwrap();
        let env: Vec<(String, String)> = proxy.env_for("job1");
        let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let script = format!(
            "/usr/bin/nc -v -z -G 5 192.0.2.1 9; echo \"out=$?\"; /usr/bin/nc -z -G 5 127.0.0.1 {port}; echo \"here=$?\"; /usr/bin/curl -sS -m 20 http://en.wikipedia.org/w/api.php; echo \"curl=$?\""
        );
        let log = d.path().join("log");
        let mut r = Running::start(
            spec(&["/bin/sh", "-c", &script]),
            1,
            &env,
            log.clone(),
            &d.path().join("job.json"),
            None,
            Some(crate::budget::SANDBOX),
        )
        .unwrap();
        let st = r.wait().unwrap();
        assert!(st.success());
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("Operation not permitted") && text.contains("out=1"), "{text}");
        assert!(text.contains("here=0"), "{text}");
        assert!(text.contains("upstream saw: GET /w/api.php HTTP/1.1") && text.contains("curl=0"), "{text}");
        assert_eq!(budget.ledger().len(), 1, "the Wikimedia request metered");
        // (curl sent its tag: its bytes counted as the job's, crate::budget::net.)
        let (got, sent) = proxy.net().unwrap().job("job1").unwrap();
        assert!(got > 0 && sent > 0, "{got} {sent}");
        drop(here);
    }

    /// A job's live view: the phase its counts file says (its name), the target under way
    /// and the ended ones its costs file says, its NAS and internet bytes; each None when not said.
    #[test]
    fn a_jobs_live_view_reads_its_phase_target_and_bytes() {
        let d = tempfile::tempdir().unwrap();
        let (counts, costs) = (d.path().join("nascounts"), d.path().join("costs.jsonl"));
        assert_eq!(Live::read(&counts, &costs, 0, None), Live::default());
        std::fs::create_dir(&counts).unwrap();
        // (A running process's file, this one's pid: its counts, then its phase.)
        let mut b = vec![0u8; 18 * 8 + 1];
        b[0] = 9;
        b[2 * 8] = 4;
        b[10 * 8] = 2;
        b[11 * 8..11 * 8 + 8].copy_from_slice(&5_000u64.to_le_bytes());
        b[12 * 8..12 * 8 + 19].copy_from_slice(b"saving records\tdisk");
        std::fs::write(counts.join(format!("{}-1", std::process::id())), &b).unwrap();
        std::fs::write(&costs, "{\"unit\":\"unit 6/1/1\",\"started\":1}\n{\"unit\":\"unit 6/1/1\",\"peak_mb\":1}\n{\"unit\":\"unit 6/1/2\",\"started\":7}\n").unwrap();
        let l = Live::read(&counts, &costs, 3, Some((10, 2)));
        assert_eq!(
            l,
            Live {
                phase: Some("saving records".into()),
                phase_since: Some(5),
                target: Some("unit 6/1/2".into()),
                target_since: Some(7),
                targets_done: 1,
                targets: 3,
                nas_read: Some(9),
                nas_wrote: Some(4),
                net_in: Some(10),
                net_out: Some(2),
            }
        );
    }

    #[test]
    fn exit_status() {
        let d = tempfile::tempdir().unwrap();
        let mut r = Running::start(
            spec(&["/bin/sh", "-c", "exit 3"]),
            1,
            &[],
            d.path().join("log"),
            &d.path().join("job.json"),
            None,
            None,
        )
        .unwrap();
        let st = loop {
            if let Some(s) = r.poll().unwrap() {
                break s;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        assert_eq!(st.code(), Some(3));
    }

    /// Task #393: a job handed over is taken over from its record by the next agent, which reads
    /// its end from its status file (not its child): its code, a pause's 75, and a kill while no
    /// agent tended it (`vanished`); one with no saved slot isn't.
    ///
    /// (The job says it's ready once its shell runs, then blocks reading a fifo: no process of its
    /// group is being started when the kill comes. A kill while its head is still forking may miss
    /// the child it is starting, which then waits on alone, its group never empty; on a loaded Mac,
    /// where a job's shell takes seconds to start, it did.)
    #[test]
    fn a_job_handed_over_is_taken_over_and_its_end_read() {
        let d = tempfile::tempdir().unwrap();
        for (code, want) in [("0", Some(0)), ("75", Some(75)), ("kill", None)] {
            let rec = d.path().join(format!("job-{code}.json"));
            let ready = d.path().join(format!("ready-{code}"));
            let go = d.path().join(format!("go-{code}"));
            let c = std::ffi::CString::new(go.to_str().unwrap()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
            // (Shell builtins only: it starts no process once ready.)
            let script = format!(
                ": > '{}'; read x < '{}'; exit {}",
                ready.display(),
                go.display(),
                if code == "kill" { "0" } else { code }
            );
            let r = Running::start(spec(&["/bin/sh", "-c", &script]), 2, &[], d.path().join("log"), &rec, None, None)
                .unwrap();
            let pgid = r.pgid;
            assert!(adopt(&rec).is_none(), "no slot saved yet");
            r.hand_over(&rec, serde_json::json!({"lease": null}));
            let (mut a, hand) = adopt(&rec).expect("taken over");
            assert_eq!(hand, serde_json::json!({"lease": null}));
            assert!(a.adopted && a.pgid == pgid && a.threads == 2 && a.spec.cmd[2] == script);
            assert!(a.poll().unwrap().is_none());
            wait_for(&mut a, "it ready", |_| ready.exists().then_some(()));
            if code == "kill" {
                // (Its status file not written: killed with its head. The head is still this
                // test's child, as it isn't once its agent has ended: reaped here, as launchd would.)
                crate::sys::signal_group(pgid, Signal::Kill);
                unsafe { libc::waitpid(pgid, std::ptr::null_mut(), 0) };
            } else {
                // (Opened once it reads, blocked till then: the job is past its `ready`.)
                std::fs::write(&go, "\n").unwrap();
            }
            let st = wait_for(&mut a, "its end", |a| a.poll().unwrap());
            assert_eq!(st.code(), want);
            assert_eq!(a.vanished, code == "kill");
            unsafe { libc::waitpid(pgid, std::ptr::null_mut(), 0) };
            assert!(wait_for(&mut a, "the group empty", |a| group_members(a.pgid).is_empty().then_some(true)));
        }
    }

    /// Task #393 review: a job that ended while no agent tended it, its group gone (its id free to
    /// be another's), is taken over as ended: its status read, no group kept (0), so stopping it
    /// signals nothing.
    #[test]
    fn a_job_ended_before_its_take_over_keeps_no_group() {
        let d = tempfile::tempdir().unwrap();
        let rec = d.path().join("job.json");
        let r =
            Running::start(spec(&["/bin/sh", "-c", "exit 4"]), 1, &[], d.path().join("log"), &rec, None, None).unwrap();
        let pgid = r.pgid;
        r.hand_over(&rec, serde_json::json!({"lease": null}));
        unsafe { libc::waitpid(pgid, std::ptr::null_mut(), 0) };
        let (mut a, _) = adopt(&rec).expect("taken over, ended");
        assert_eq!(a.pgid, 0);
        assert!(a.caffeinate.is_none());
        assert_eq!(a.poll().unwrap().and_then(|s| s.code()), Some(4));
        a.stop(Duration::from_secs(1));
    }

    #[test]
    fn orphans_are_stopped() {
        let d = tempfile::tempdir().unwrap();
        let rec = d.path().join("job.json");
        // The shell leaves a sleep behind in its group and exits: an orphan, as after a crash.
        let mut r = Running::start(
            spec(&["/bin/sh", "-c", "/bin/sleep 3600 & echo started"]),
            1,
            &[],
            d.path().join("log"),
            &rec,
            None,
            None,
        )
        .unwrap();
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
