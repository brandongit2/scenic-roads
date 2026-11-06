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

/// A job's id: its step (a task's kind for a leased task, crate::agent::steps::TASKS), then what
/// it's for ("unit 6/31/20", "terrain-hi 3/4/2 3/4/3", "osm-pass 2026-10-01", "tail 6/1/1"); a job
/// of no targets its step alone ("gc"). Written by its `Display`, read back by `parse` alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobId<'a> {
    pub step: &'a str,
    pub rest: Option<&'a str>,
}

impl<'a> JobId<'a> {
    pub fn parse(id: &'a str) -> Self {
        match id.split_once(' ') {
            Some((step, rest)) => JobId { step, rest: Some(rest) },
            None => JobId { step: id, rest: None },
        }
    }

    /// What it's for: its rest, or for a job of its step alone, its step again (GC's cost key:
    /// "gc gc").
    pub fn target(&self) -> &'a str {
        self.rest.unwrap_or(self.step)
    }
}

impl std::fmt::Display for JobId<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.rest {
            Some(r) => write!(f, "{} {r}", self.step),
            None => f.write_str(self.step),
        }
    }
}
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus, Stdio};
use std::time::{Duration, Instant};

// A job's side (its progress lines, its log read back): the runtime crate's (task #916 step 3,
// docs/crates.md), kept at these paths.
use jobrt::jobs::RUN_START;
pub use jobrt::jobs::{
    error_of, now_s, part, parts, progress, report, report_f, run_watched, said, stage, tail, unit_key, within, Said,
};

/// The NAS project folder as a child process's `--root` argument (its spec's): the spawners' one
/// way out of the type (tests/nas_access.rs).
pub fn root_arg(root: &store::nasfs::NasRoot) -> String {
    root.unchecked_path().to_string_lossy().into_owned()
}

/// Notes what a job's run of `target` of `step` cost in its costs file (`SCENIC_COSTS`, as
/// scenic-build's steps do; nothing outside a job): the most memory its processes held (sampled,
/// crate::sys::group_peak), `secs`, its step's way (crate::coord::cost_version) and its inputs'
/// measures (crate::costin). The clean-up's (crate::agent::gc) and a redo's (crate::agent::redo),
/// run by the `scenic` program.
pub fn note_cost(step: &str, target: &str, secs: u64) {
    let Some(p) = std::env::var_os("SCENIC_COSTS") else { return };
    let line = serde_json::json!({
        "unit": crate::coord::cost_key(step, target),
        "peak_mb": crate::sys::group_peak() >> 20,
        "secs": secs,
        "v": crate::coord::cost_version(step),
        "sizes": crate::costin::take(),
    });
    let r = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
        .and_then(|mut f| std::io::Write::write_all(&mut f, format!("{line}\n").as_bytes()));
    if let Err(e) = r {
        eprintln!("{step}: noting what it cost: {e}");
    }
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
/// its NAS bytes read and written, files made and metadata work (its processes' counts,
/// store::nasio: the lead weighs a job past its predicted time by them, crate::coord::sched's
/// `job_weight`) and its internet bytes in
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
    /// The files it made on the NAS, and its metadata work there (store::nasio's counts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nas_creates: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nas_listed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nas_stats: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nas_deletes: Option<u64>,
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
            nas_creates: at(store::nasio::Count::Creates),
            nas_listed: at(store::nasio::Count::Listed),
            nas_stats: at(store::nasio::Count::Stats),
            nas_deletes: at(store::nasio::Count::Deletes),
            net_in: net.map(|n| n.0),
            net_out: net.map(|n| n.1),
        }
    }

    /// What its job has moved on the NAS since it began: bytes read and written, files made,
    /// entries listed, files statted and deleted. None unless it says all of them: an older agent's
    /// beat says its bytes alone, and weighing a job by its bytes alone would miss its files made.
    pub fn nas_moved(&self) -> Option<[u64; 5]> {
        Some([
            self.nas_read? + self.nas_wrote?,
            self.nas_creates?,
            self.nas_listed?,
            self.nas_stats?,
            self.nas_deletes?,
        ])
    }

    /// Its job's mean NAS rate (crate::budget::nas::Rate) over the `secs` since `base` (an earlier
    /// beat's view of the same job; None: since it began): what it moved over that time; None when
    /// either says not all it moved (`nas_moved`).
    pub fn nas_rate(&self, base: Option<&Live>, secs: f64) -> Option<crate::budget::nas::Rate> {
        let now = self.nas_moved()?;
        let then = match base {
            Some(b) => b.nas_moved()?,
            None => [0; 5],
        };
        let s = secs.max(1.0);
        let per = |i: usize| now[i].saturating_sub(then[i]) as f64 / s;
        Some(crate::budget::nas::Rate {
            bytes_s: per(0),
            creates_s: per(1),
            listed_s: per(2),
            stats_s: per(3),
            deletes_s: per(4),
        })
    }
}

/// One step for some targets, with the key each target will be recorded under.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Work {
    /// "terrain", "slope", "unit", "pack", "lo", "catalog".
    pub step: String,
    pub targets: Vec<(String, String)>,
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
    pub record: Option<Work>,
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
    /// The worker threads it was given (RAYON_NUM_THREADS), then as re-granted (its threads file,
    /// crate::control::THREADS_ENV: docs/pool.md §7.1, Threads re-granted).
    pub threads: usize,
    /// Since when its targets run on the threads they do now, its CPU time (ns) then, and those
    /// threads, as the agent last saw them (`Agent::tend_grant`): what it used on them, for its lead's
    /// re-grant. None while it doesn't say (a job not taking a re-grant) or is stopped.
    pub on_threads: Option<(Instant, u64, u32)>,
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
    work: Option<Work>,
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
            on_threads: None,
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
    /// after `grace` to whatever of it is left. Its group when some of it outlived the SIGKILL, for
    /// the agent to kill again each round until it's gone (`Group::kill_again`).
    pub fn stop(&mut self, grace: Duration) -> Option<Group> {
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
        let g = self.group();
        let left = g.kill_again();
        if left {
            eprintln!(
                "agent: {}'s group {} outlived its SIGKILL: killed again each round until it's gone",
                g.id, g.pgid
            );
        }
        left.then_some(g)
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
            on_threads: None,
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
    pub done: Option<(Work, PathBuf)>,
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
    /// member lives, and is once none does). Its members as they run: one ended and not yet reaped
    /// (whose start can't be read) is none, so a group of those alone is gone, and one beside a
    /// running member doesn't hide it (task #987).
    pub fn is_the_jobs(&self) -> bool {
        let members: Vec<i32> = if self.pgid > 1 { group_members(self.pgid) } else { Vec::new() };
        let members: Vec<i32> = members.into_iter().filter(|&p| process_start(p).is_some()).collect();
        !members.is_empty()
            && match process_start(self.pgid) {
                Some(t) => self.leader_start != 0 && t == self.leader_start,
                None => members.iter().all(|&p| process_start(p).is_some_and(|t| t + 2 >= self.started)),
            }
    }

    /// SIGKILL to the group again while it's still the job's; whether any of it was left. A killed
    /// group can outlive its SIGKILL: a member's fork under way as the signal went out finishes
    /// after it, its child in the group never signalled (or signalled while still being made, the
    /// signal lost; task #987: 56 groups in 12,600 killed as they started, the M4 under load).
    /// The forking member dies as its fork returns, and a SIGKILL sent after the fork reaches the
    /// child: each round's reaches what the one before missed, until the group is gone.
    pub fn kill_again(&self) -> bool {
        let left = self.is_the_jobs();
        if left {
            crate::sys::signal_group(self.pgid, Signal::Kill);
        }
        left
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
        crate::control::tell(&control, crate::control::Say::Run).unwrap();
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
        crate::control::tell(&control, crate::control::Say::Drain).unwrap();
        let st = wait_for(&mut r, "stopped at its next safe point", |r| r.poll().unwrap());
        assert_eq!(st.code(), Some(crate::control::PAUSED_EXIT));
        assert_eq!(crate::control::read_done(&done, "unit"), ["6/1/1", "6/1/2", "6/1/3"]);
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
    /// and the ended ones its costs file says, its NAS bytes and files made and its internet bytes;
    /// each None when not said.
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
        b[3 * 8] = 3;
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
                nas_creates: Some(3),
                nas_listed: Some(0),
                nas_stats: Some(0),
                nas_deletes: Some(0),
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

    /// Task #987: what of a group outlives its SIGKILL (a fork under way as it went out) is killed
    /// again, its ended processes not yet reaped counting for nothing.
    #[test]
    fn a_group_left_after_its_kill_is_killed_again() {
        let d = tempfile::tempdir().unwrap();
        let live = |pgid: i32| group_members(pgid).into_iter().filter(|&p| process_start(p).is_some()).count();
        // A job (its head's shell, the command's, two sleeps) stopped once all of it runs (no fork
        // under way): nothing left.
        let cmd = "/bin/sleep 3600 & /bin/sleep 3600; wait";
        let mut r = Running::start(
            spec(&["/bin/sh", "-c", cmd]),
            1,
            &[],
            d.path().join("log"),
            &d.path().join("job.json"),
            None,
            None,
        )
        .unwrap();
        let pgid = r.pgid;
        let sleeps = |pgid: i32| {
            group_members(pgid).into_iter().filter_map(crate::sys::process).filter(|p| p.exe == "sleep").count()
        };
        store::testkit::eventually("its two sleeps", || sleeps(pgid) == 2);
        assert_eq!(r.stop(Duration::from_secs(1)), None);
        // A group whose head has ended (unreaped: not running, its start unreadable) beside a
        // sleep it left: still the job's, killed again, then gone.
        let mut c = store::spawn::command("/bin/sh");
        c.args(["-c", "/bin/sleep 3600 & exit 0"]);
        crate::sys::own_group(&mut c);
        let mut head = store::spawn::spawn(&mut c).unwrap();
        let pgid = head.id() as i32;
        // (Its head's start not kept: it may have ended already.)
        let g = Group { pgid, leader_start: 0, started: now_s() - 5, id: "left".into() };
        store::testkit::eventually("its head ended, the sleep left", || {
            process_start(pgid).is_none() && live(pgid) == 1
        });
        assert!(g.kill_again());
        store::testkit::eventually("the sleep gone", || live(pgid) == 0);
        assert!(!g.kill_again());
        head.wait().unwrap();
    }
}
