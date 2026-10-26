//! Pausing the build (docs/building.md §2, Pausing): what a pause is, how a Mac asks for one, and the
//! channel a running job hears it on.
//!
//! One pause holds for every machine. The lead's coordinator keeps it (crate::coord: on its
//! disk, mirrored to the NAS) and tells its workers, in its answers to their asks and beats. Each
//! Mac's agent asks for a pause, or for the build to go on, as that Mac's menu, `scenic pause` or the
//! map says (`Request`: a file in the agent's folder, taken up within seconds), and passes it on to
//! the coordinator; a helper that can't reach it pauses itself meanwhile, and one that last heard
//! the build was paused stays paused until it hears otherwise.
//!
//! A pause stops the work at its next safe point (`Mode::Drain`): the running job finishes the
//! target it's on (a z3 tile, a z6 tile, a map tile), saves it, notes it done (`done`) and ends
//! (`PAUSED_EXIT`); nothing new starts. The terrain's steps stop sooner, between their sampling's
//! chunks, the target they're on unsaved but for its resume log (`Paused`). A job that hasn't
//! reached a safe point within the agent's grace is frozen where it is instead, and goes on from
//! there. Or at once (`Mode::Freeze`): the job frozen where it is. Either way no target finished is
//! lost, and nothing is counted as a failure.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

/// How a pause stops the work.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// At the next safe point (a job ends once the target it's on is saved), else frozen after the
    /// agent's grace.
    Drain,
    /// At once: a job frozen where it is.
    Freeze,
}

/// The build's pause: how, who asked, and since when.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pause {
    pub mode: Mode,
    /// Who asked, in words: "the menu bar on Brandons-MacBook-Pro", "scenic pause on …", "the map".
    pub by: String,
    /// Unix seconds.
    pub at: u64,
}

impl Pause {
    pub fn new(mode: Mode, by: &str) -> Pause {
        Pause { mode, by: by.to_string(), at: now() }
    }

    /// Why the work waits, for the status.
    pub fn why(&self) -> String {
        format!("the build is paused (from {})", self.by)
    }
}

/// A Mac's ask, from its menu, `scenic pause` or the map: the build paused (`Some`), or going on
/// (`None`). Kept in its agent's folder (`REQUEST`, replaced by a later ask) until the agent has
/// passed it on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub pause: Option<Pause>,
    /// When it was asked (unix seconds).
    pub at: u64,
}

/// The ask's file, in the agent's folder.
pub const REQUEST: &str = "pause-request.json";

/// Asks this Mac's agent (its folder `home`) to pause the build, or let it go on.
pub fn request(home: &Path, pause: Option<Pause>) -> anyhow::Result<()> {
    std::fs::create_dir_all(home)?;
    store::whole::write(&home.join(REQUEST), &serde_json::to_vec(&Request { pause, at: now() })?)
}

/// The ask waiting in `home`, if any (one that doesn't parse: none, and it goes).
pub fn take_request(home: &Path) -> Option<Request> {
    let p = home.join(REQUEST);
    let b = std::fs::read(&p).ok()?;
    let r = serde_json::from_slice(&b).ok();
    if r.is_none() {
        std::fs::remove_file(&p).ok();
    }
    r
}

/// The ask waiting in `home`, if any, read only (one that doesn't parse: none, left for the agent).
pub fn peek_request(home: &Path) -> Option<Request> {
    serde_json::from_slice(&std::fs::read(home.join(REQUEST)).ok()?).ok()
}

/// The ask passed on: it goes, unless a newer one came meanwhile.
pub fn clear_request(home: &Path, r: &Request) {
    if take_request(home).as_ref() == Some(r) {
        std::fs::remove_file(home.join(REQUEST)).ok();
    }
}

/// An ask of the pool's lead (docs/pool.md §6.3, §6.5, §11), from this Mac's menu, `scenic lead`,
/// the map or the build page: hand the lead to a member (`give`: its member id, or its host name),
/// or have this Mac take it over (`take`: the owner's force and downgrade, from this Mac's menu or
/// `scenic lead take` alone).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum LeadAsk {
    Give {
        to: String,
    },
    Take {
        #[serde(default)]
        force: bool,
        #[serde(default)]
        downgrade: bool,
    },
}

/// A lead ask, who asked (in words) and when: kept in the agent's folder (`LEAD_REQUEST`, replaced
/// by a later ask) until the agent takes it up, at its next loop.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeadRequest {
    pub ask: LeadAsk,
    #[serde(default)]
    pub by: String,
    #[serde(default)]
    pub at: u64,
}

/// The lead ask's file, in the agent's folder.
pub const LEAD_REQUEST: &str = "lead-request.json";

/// Asks this Mac's agent (its folder `home`) to hand the lead over, or take it.
pub fn request_lead(home: &Path, ask: LeadAsk, by: &str) -> anyhow::Result<LeadRequest> {
    std::fs::create_dir_all(home)?;
    let r = LeadRequest { ask, by: by.to_string(), at: now() };
    store::whole::write(&home.join(LEAD_REQUEST), &serde_json::to_vec(&r)?)?;
    Ok(r)
}

/// The lead ask waiting in `home`, taken (its file removed; one that doesn't parse: none, and it
/// goes).
pub fn take_lead(home: &Path) -> Option<LeadRequest> {
    let p = home.join(LEAD_REQUEST);
    let b = std::fs::read(&p).ok()?;
    std::fs::remove_file(&p).ok();
    serde_json::from_slice(&b).ok()
}

/// The lead ask waiting in `home`, read only.
pub fn peek_lead(home: &Path) -> Option<LeadRequest> {
    serde_json::from_slice(&std::fs::read(home.join(LEAD_REQUEST)).ok()?).ok()
}

/// How the owner quits Scenic (the app, tools/status) on this Mac with its build service: the jobs
/// under way finish first (`Finish`: nothing new taken meanwhile), or stop now (`Now`: each given
/// back, what it finished handed off, its scratch kept on this Mac's disk). Either way a leading
/// Mac then hands the lead to a member that can take it, and the agent exits with `QUIT_EXIT`,
/// which its launcher passes on as a clean exit, so launchd leaves it stopped until the app is
/// opened again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuitMode {
    Finish,
    Now,
}

/// The app's ask to quit (`QUIT_REQUEST`): kept in the agent's folder while the agent quits (an
/// agent restarted meanwhile goes on quitting), removed by the agent as it exits, and by the app
/// when it's opened again (which cancels it).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quit {
    pub mode: QuitMode,
    /// Who asked, in words: "Scenic on m4", "Scenic on m4, logging out".
    #[serde(default)]
    pub by: String,
    #[serde(default)]
    pub at: u64,
}

/// The quit ask's file, in the agent's folder.
pub const QUIT_REQUEST: &str = "quit-request.json";

/// How the agent exits once it has quit as asked: its launcher then exits cleanly, not starting it
/// again (tools/launcher/launcher.c).
pub const QUIT_EXIT: i32 = 76;

/// Asks this Mac's agent (its folder `home`) to quit.
pub fn request_quit(home: &Path, mode: QuitMode, by: &str) -> anyhow::Result<Quit> {
    std::fs::create_dir_all(home)?;
    let q = Quit { mode, by: by.to_string(), at: now() };
    store::whole::write(&home.join(QUIT_REQUEST), &serde_json::to_vec(&q)?)?;
    Ok(q)
}

/// The quit ask waiting in `home`, if any (one that doesn't parse: none, left as it is).
pub fn peek_quit(home: &Path) -> Option<Quit> {
    serde_json::from_slice(&std::fs::read(home.join(QUIT_REQUEST)).ok()?).ok()
}

/// The quit ask done: it goes.
pub fn clear_quit(home: &Path) {
    std::fs::remove_file(home.join(QUIT_REQUEST)).ok();
}

/// The owner's cancel of a running job (`scenic job cancel <target> [--rerun-after-update]`, the
/// menu bar): the job building `target` (of `step`, when it names one) stops now and gives its work
/// back, not failed (no retry delay, nothing kept from its Mac), its in-progress work dropped (its
/// scratch for the target cleared; targets it finished are kept). With `rerun_after_update` the
/// target is held until the lead runs a newer app than the one it was cancelled on
/// (crate::coord::lease::Hold), so the Mac that takes it then runs the newer; an update waiting on
/// that job restarts at once, its slot free. On the lead, or a member: a job on another Mac is told
/// through the lead's coordinator, in its beat's answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cancel {
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    #[serde(default)]
    pub rerun_after_update: bool,
    /// Who asked, in words: "scenic job cancel on m4".
    #[serde(default)]
    pub by: String,
    #[serde(default)]
    pub at: u64,
}

/// A build page's ask to try a failed target again now (docs/pool.md §11): the lead forgets its
/// failures, the coordinator's (`failed`: no worker held off it) and its agent's own back-off.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Retry {
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    /// Who asked, in words: "the build page (m1)".
    #[serde(default)]
    pub by: String,
    #[serde(default)]
    pub at: u64,
}

impl Cancel {
    pub fn new(target: &str, step: Option<&str>, rerun_after_update: bool, by: &str) -> Cancel {
        Cancel {
            target: target.to_string(),
            step: step.map(str::to_string),
            rerun_after_update,
            by: by.to_string(),
            at: now(),
        }
    }

    /// The cancel in words, for the logs, the history and the status.
    pub fn said(&self) -> String {
        format!(
            "cancelled by {}{}",
            self.by,
            if self.rerun_after_update { "; reruns once the lead runs a newer app" } else { "; reruns" }
        )
    }
}

/// The cancel asks' folder, in the agent's folder: a file each (`<anything>.json`, written whole by
/// its asker, never rewritten), taken up at the agent's next loop. One file an ask, so no ask is
/// lost to, or done twice by, another written while the agent takes them.
pub const CANCEL_REQUESTS: &str = "cancel-requests";

/// How long a cancel ask stands (s, by its `at`): an older one, an agent that wasn't running then,
/// is dropped unread, so it can't stop a job begun since.
pub const CANCEL_STALE_S: u64 = 120;

/// The agent's answer to the last cancel it took up (`CancelAnswer`), in its folder.
pub const CANCEL_ANSWER: &str = "cancel-answer.json";

/// What the agent did with a cancel: the ask, and what it stopped, in words.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelAnswer {
    pub ask: Cancel,
    pub said: String,
    /// Whether a job was stopped or told to stop.
    pub hit: bool,
}

/// Asks this Mac's agent (its folder `home`) to cancel a job: a file of its own in
/// `CANCEL_REQUESTS`.
pub fn request_cancel(home: &Path, c: &Cancel) -> anyhow::Result<()> {
    let dir = home.join(CANCEL_REQUESTS);
    std::fs::create_dir_all(&dir)?;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let name = format!("{nanos}-{}.json", std::process::id());
    store::whole::write(&dir.join(name), &serde_json::to_vec(c)?)
}

/// The cancel asks' files waiting in `home`, oldest first.
fn cancel_files(home: &Path) -> Vec<std::path::PathBuf> {
    let mut v: Vec<std::path::PathBuf> = std::fs::read_dir(home.join(CANCEL_REQUESTS))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    v.sort();
    v
}

/// The cancel asks waiting in `home`, read only.
pub fn peek_cancels(home: &Path) -> Vec<Cancel> {
    cancel_files(home).iter().filter_map(|p| serde_json::from_slice(&std::fs::read(p).ok()?).ok()).collect()
}

/// The cancel asks waiting in `home`, taken (each file read whole, then removed): those asked within
/// `CANCEL_STALE_S` of `now` (unix seconds); older ones, and ones that don't parse, go unread.
pub fn take_cancels(home: &Path, now: u64) -> Vec<Cancel> {
    let mut out = Vec::new();
    for p in cancel_files(home) {
        let c: Option<Cancel> = std::fs::read(&p).ok().and_then(|b| serde_json::from_slice(&b).ok());
        if std::fs::remove_file(&p).is_err() {
            continue;
        }
        match c {
            Some(c) if now.saturating_sub(c.at) <= CANCEL_STALE_S => out.push(c),
            Some(c) => {
                eprintln!("agent: a cancel of {} asked {} s ago dropped (stale)", c.target, now.saturating_sub(c.at))
            }
            None => {}
        }
    }
    out
}

/// The agent's answer to a cancel, kept for `scenic job cancel` to print.
pub fn answer_cancel(home: &Path, a: &CancelAnswer) {
    let w = serde_json::to_vec(a)
        .map_err(anyhow::Error::from)
        .and_then(|b| store::whole::write(&home.join(CANCEL_ANSWER), &b));
    if let Err(e) = w {
        eprintln!("agent: writing the cancel's answer: {e:#}");
    }
}

/// The agent's answer to cancel `c`, once it has taken it up.
pub fn cancel_answer(home: &Path, c: &Cancel) -> Option<CancelAnswer> {
    let a: CancelAnswer = serde_json::from_slice(&std::fs::read(home.join(CANCEL_ANSWER)).ok()?).ok()?;
    (a.ask == *c).then_some(a)
}

/// The running job's channel: a file the agent writes ("run" or "drain"), named by this variable.
pub const CONTROL_ENV: &str = "SCENIC_CONTROL";

/// The targets a job finished, a line each (`<step> <target>`): the agent records them whatever
/// becomes of the rest (paused, failed or stopped), so they aren't built again.
pub const DONE_ENV: &str = "SCENIC_DONE";

/// How a job that stopped at a safe point because the build is pausing exits (EX_TEMPFAIL): not a
/// failure.
pub const PAUSED_EXIT: i32 = 75;

/// Whether the build is pausing: the running job stops at its next safe point.
pub fn draining() -> bool {
    std::env::var_os(CONTROL_ENV).and_then(|p| std::fs::read(p).ok()).is_some_and(|b| b.starts_with(b"drain"))
}

/// A safe point of `step`: with the build pausing, the job ends here, as paused (each target before
/// it saved and noted done as it finished).
pub fn safe_point(step: &str) {
    if draining() {
        stop_paused(step);
    }
}

/// What a long piece of work between a step's safe points returns when the build is pausing
/// (`pause_point`): the terrain's sampling of the elevation pool checks between its chunks
/// (crate::terrain_base::Tiles), as a level's sampling can run for hours. The step catches it at
/// its own safe point, once its uploads in flight have landed, and stops there as a pause does
/// (`stop_if_paused`): the target it was on not saved, its resume log (crate::terrain_resume)
/// kept.
#[derive(Debug)]
pub struct Paused;

impl std::fmt::Display for Paused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the build is pausing: stopped between chunks")
    }
}

impl std::error::Error for Paused {}

/// Err(`Paused`) when `draining` says the build is pausing.
pub fn pause_point(draining: &dyn Fn() -> bool) -> anyhow::Result<()> {
    if draining() {
        return Err(Paused.into());
    }
    Ok(())
}

/// Whether `e` is (or wraps) a `Paused`.
pub fn is_paused(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.is::<Paused>())
}

/// `e`, unless it's a pause (`is_paused`): then the job ends as paused here (`stop_paused`). Called
/// at `step`'s safe point, its uploads in flight landed.
pub fn stop_if_paused(step: &str, e: anyhow::Error) -> anyhow::Error {
    if is_paused(&e) {
        stop_paused(step);
    }
    e
}

/// Ends the job as paused, at a safe point it stopped at (each target before it saved and noted
/// done): whether or not the pause has been lifted meanwhile, as it didn't do the rest.
pub fn stop_paused(step: &str) -> ! {
    eprintln!("{step}: paused at a safe point; the rest goes on when the build does");
    // (Its timings kept, as a run that didn't finish.)
    crate::timings::finish(false);
    std::process::exit(PAUSED_EXIT)
}

/// Notes `target` of `step` done: its outputs are saved (in the manifest, or the hand-off).
pub fn done(step: &str, target: &str) {
    let Some(p) = std::env::var_os(DONE_ENV) else { return };
    let r = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
        .and_then(|mut f| f.write_all(format!("{step} {target}\n").as_bytes()));
    if let Err(e) = r {
        eprintln!("{step} {target}: noting it done: {e}");
    }
}

/// The targets of `step` a job noted done (`done`), in order, once each.
pub fn read_done(path: &Path, step: &str) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut out: Vec<String> = Vec::new();
    for l in text.lines() {
        if let Some((s, t)) = l.split_once(' ') {
            if s == step && !out.iter().any(|x| x == t) {
                out.push(t.to_string());
            }
        }
    }
    out
}

/// One Mac paused alone (task #224; docs/pool.md §11): its jobs stop at their next safe point and
/// are given back, it takes no new work and can't take the lead anew (leading, it keeps the lead
/// and goes on granting the members' work), and keeps beating; the rest of the pool goes on. Who asked, in words, and when (unix seconds).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hold {
    pub by: String,
    pub at: u64,
}

impl Hold {
    pub fn new(by: &str) -> Hold {
        Hold { by: by.to_string(), at: now() }
    }

    /// Why this Mac's work waits, for the status.
    pub fn why(&self) -> String {
        format!("this Mac is paused (from {}); the rest of the pool goes on", self.by)
    }

    /// The hold as a pause of this Mac's own jobs: at their next safe point.
    pub fn as_pause(&self) -> Pause {
        Pause { mode: Mode::Drain, by: self.by.clone(), at: self.at }
    }
}

/// A Mac's hold as last set (`held`: paused, else going on) and when (`at`, unix seconds): the
/// later of two settings wins. Kept in its agent's folder (`HOLD`) so a restart keeps it, and on
/// the NAS (`hold_path`), where another Mac's `scenic pause <member>` sets it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldState {
    #[serde(default)]
    pub held: Option<Hold>,
    #[serde(default)]
    pub at: u64,
}

/// The hold's file in the agent's folder.
pub const HOLD: &str = "hold.json";

/// A member's hold on the NAS, by member id.
pub fn hold_path(member: &str) -> String {
    format!("state/pool/holds/{member}.json")
}

/// This Mac's hold as its agent keeps it (none, or a file that doesn't parse: going on).
pub fn read_hold(home: &Path) -> HoldState {
    std::fs::read(home.join(HOLD)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// An ask to pause one Mac or let it go on (`held`), from `scenic pause --here|<member>` or the
/// menu bar: this Mac (`member` None) or another member (its id or host name). Kept in the agent's
/// folder (`HOLD_REQUEST`, replaced by a later ask) until the agent takes it up.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    #[serde(default)]
    pub held: Option<Hold>,
    #[serde(default)]
    pub at: u64,
}

/// The hold ask's file, and the agent's answer to the last one it took (`HoldAnswer`).
pub const HOLD_REQUEST: &str = "hold-request.json";
pub const HOLD_ANSWER: &str = "hold-answer.json";

/// What the agent did with a hold ask, in words, and whether it was done.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldAnswer {
    pub ask: HoldRequest,
    pub said: String,
    pub ok: bool,
}

/// Asks this Mac's agent (its folder `home`) to pause a Mac alone, or let it go on.
pub fn request_hold(home: &Path, member: Option<&str>, held: Option<Hold>) -> anyhow::Result<HoldRequest> {
    std::fs::create_dir_all(home)?;
    let r = HoldRequest { member: member.map(str::to_string), held, at: now() };
    store::whole::write(&home.join(HOLD_REQUEST), &serde_json::to_vec(&r)?)?;
    Ok(r)
}

/// The hold ask waiting in `home`, taken (its file removed; one that doesn't parse: none).
pub fn take_hold_request(home: &Path) -> Option<HoldRequest> {
    let p = home.join(HOLD_REQUEST);
    let b = std::fs::read(&p).ok()?;
    std::fs::remove_file(&p).ok();
    serde_json::from_slice(&b).ok()
}

/// The hold ask waiting in `home`, read only.
pub fn peek_hold_request(home: &Path) -> Option<HoldRequest> {
    serde_json::from_slice(&std::fs::read(home.join(HOLD_REQUEST)).ok()?).ok()
}

/// The agent's answer to a hold ask, kept for `scenic pause` to print.
pub fn answer_hold(home: &Path, a: &HoldAnswer) {
    let w = serde_json::to_vec(a)
        .map_err(anyhow::Error::from)
        .and_then(|b| store::whole::write(&home.join(HOLD_ANSWER), &b));
    if let Err(e) = w {
        eprintln!("agent: writing the hold's answer: {e:#}");
    }
}

/// The agent's answer to hold ask `r`, once it has taken it up.
pub fn hold_answer(home: &Path, r: &HoldRequest) -> Option<HoldAnswer> {
    let a: HoldAnswer = serde_json::from_slice(&std::fs::read(home.join(HOLD_ANSWER)).ok()?).ok()?;
    (a.ask == *r).then_some(a)
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_and_done_lists() {
        let d = tempfile::tempdir().unwrap();
        assert!(take_request(d.path()).is_none());
        request(d.path(), Some(Pause::new(Mode::Drain, "the menu bar on m4"))).unwrap();
        let r = take_request(d.path()).unwrap();
        assert_eq!(r.pause.as_ref().map(|p| (p.mode, p.by.as_str())), Some((Mode::Drain, "the menu bar on m4")));
        // Passed on: it goes; a newer ask meanwhile stays.
        request(d.path(), None).unwrap();
        clear_request(d.path(), &r);
        assert!(take_request(d.path()).is_some_and(|x| x.pause.is_none()));
        let r2 = take_request(d.path()).unwrap();
        clear_request(d.path(), &r2);
        assert!(take_request(d.path()).is_none());
        // A lead ask: the menu's JSON as written by hand, taken once.
        std::fs::write(
            d.path().join(LEAD_REQUEST),
            r#"{"ask": {"kind": "take", "force": true}, "by": "the menu bar on m1", "at": 5}"#,
        )
        .unwrap();
        assert_eq!(peek_lead(d.path()).map(|r| r.ask), Some(LeadAsk::Take { force: true, downgrade: false }));
        assert_eq!(
            take_lead(d.path()).map(|r| (r.ask, r.at)),
            Some((LeadAsk::Take { force: true, downgrade: false }, 5))
        );
        assert!(take_lead(d.path()).is_none());
        request_lead(d.path(), LeadAsk::Give { to: "MacBook-Air".into() }, "scenic lead on m1").unwrap();
        assert_eq!(take_lead(d.path()).map(|r| r.ask), Some(LeadAsk::Give { to: "MacBook-Air".into() }));
        // A damaged one goes.
        std::fs::write(d.path().join(REQUEST), b"{").unwrap();
        assert!(take_request(d.path()).is_none() && !d.path().join(REQUEST).exists());
        // The done list: a step's targets, once each, in order.
        let p = d.path().join("done");
        std::fs::write(&p, "unit 6/1/1\nunit 6/1/2\npack 6/1/1\nunit 6/1/1\nbroken\n").unwrap();
        assert_eq!(read_done(&p, "unit"), ["6/1/1", "6/1/2"]);
        assert!(read_done(&d.path().join("none"), "unit").is_empty());
        // A quit ask: the app's JSON as it writes it, read until it's cleared.
        assert!(peek_quit(d.path()).is_none());
        std::fs::write(d.path().join(QUIT_REQUEST), r#"{"mode": "finish", "by": "Scenic on m1", "at": 7}"#).unwrap();
        assert_eq!(peek_quit(d.path()).map(|q| (q.mode, q.at)), Some((QuitMode::Finish, 7)));
        request_quit(d.path(), QuitMode::Now, "scenic on m4").unwrap();
        assert_eq!(peek_quit(d.path()).map(|q| q.mode), Some(QuitMode::Now));
        clear_quit(d.path());
        assert!(peek_quit(d.path()).is_none());
    }

    /// The menu bar's hold ask (tools/status/main.swift holdRequestJSON, task #628): the files its
    /// --test checks it against are what request_hold writes, field for field, and read back as asked.
    #[test]
    fn the_menu_bars_hold_ask_is_scenics() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/status/test");
        for (f, held) in [("hold-request-pause.json", true), ("hold-request-resume.json", false)] {
            let menu: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join(f)).unwrap()).unwrap();
            let d = tempfile::tempdir().unwrap();
            let mut ours =
                request_hold(d.path(), None, held.then(|| Hold { by: "the menu bar on m1".into(), at: 1700000000 }))
                    .unwrap();
            ours.at = 1700000000;
            assert_eq!(serde_json::to_value(&ours).unwrap(), menu, "{f}");
            std::fs::copy(dir.join(f), d.path().join(HOLD_REQUEST)).unwrap();
            assert_eq!(take_hold_request(d.path()), Some(ours), "{f}");
        }
    }

    /// Hold asks (task #224): taken once, answered by the ask they answer.
    #[test]
    fn hold_asks_and_answers() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(read_hold(d.path()), HoldState::default());
        let r = request_hold(d.path(), Some("m4"), Some(Hold::new("scenic pause on m1"))).unwrap();
        assert_eq!(take_hold_request(d.path()), Some(r.clone()));
        assert!(take_hold_request(d.path()).is_none());
        assert!(hold_answer(d.path(), &r).is_none());
        answer_hold(d.path(), &HoldAnswer { ask: r.clone(), said: "m4 is paused".into(), ok: true });
        assert!(hold_answer(d.path(), &r).is_some_and(|a| a.ok));
        assert_eq!(Hold::new("x").as_pause().mode, Mode::Drain);
    }

    /// Cancel asks (task #373): a file each, taken once in order; one older than CANCEL_STALE_S
    /// dropped unread, and a file that doesn't parse goes.
    #[test]
    fn cancel_asks_are_taken_once_and_stale_ones_dropped() {
        let d = tempfile::tempdir().unwrap();
        let a = Cancel::new("6/1/1", None, false, "x");
        let b = Cancel { at: a.at - CANCEL_STALE_S - 1, ..Cancel::new("6/1/2", Some("unit"), true, "y") };
        request_cancel(d.path(), &a).unwrap();
        request_cancel(d.path(), &b).unwrap();
        std::fs::write(d.path().join(CANCEL_REQUESTS).join("bad.json"), "{").unwrap();
        assert_eq!(peek_cancels(d.path()).len(), 2);
        assert_eq!(take_cancels(d.path(), a.at), vec![a.clone()]);
        assert!(take_cancels(d.path(), a.at).is_empty() && peek_cancels(d.path()).is_empty());
    }
}
