//! Pausing the build (docs/plan.md §8, Pausing): what a pause is, how a Mac asks for one, and the
//! channel a running job hears it on.
//!
//! One pause holds for every machine. The build Mac's coordinator keeps it (crate::coord: on its
//! disk, mirrored to the NAS) and tells its workers, in its answers to their asks and beats. Each
//! Mac's agent asks for a pause, or for the build to go on, as that Mac's menu, `scenic pause` or the
//! map says (`Request`: a file in the agent's folder, taken up within seconds), and passes it on to
//! the coordinator; a helper that can't reach it pauses itself meanwhile, and one that last heard
//! the build was paused stays paused until it hears otherwise.
//!
//! A pause stops the work at its next safe point (`Mode::Drain`): the running job finishes the
//! target it's on (an area, a map tile, a terrain area), saves it, notes it done (`done`) and ends
//! (`PAUSED_EXIT`); nothing new starts. A job that hasn't reached a safe point within the agent's
//! grace is frozen where it is instead, and goes on from there. Or at once (`Mode::Freeze`): the job
//! frozen where it is. Either way nothing done is lost, and nothing is counted as a failure.

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
    crate::whole::write(&home.join(REQUEST), &serde_json::to_vec(&Request { pause, at: now() })?)
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
    Give { to: String },
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
    crate::whole::write(&home.join(LEAD_REQUEST), &serde_json::to_vec(&r)?)?;
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

/// Ends the job as paused, at a safe point it stopped at (each target before it saved and noted
/// done): whether or not the pause has been lifted meanwhile, as it didn't do the rest.
pub fn stop_paused(step: &str) -> ! {
    eprintln!("{step}: paused at a safe point; the rest goes on when the build does");
    std::process::exit(PAUSED_EXIT)
}

/// Notes `target` of `step` done: its outputs are saved (in the manifest, or the hand-off).
pub fn done(step: &str, target: &str) {
    let Some(p) = std::env::var_os(DONE_ENV) else { return };
    let r = std::fs::OpenOptions::new().create(true).append(true).open(&p).and_then(|mut f| f.write_all(format!("{step} {target}\n").as_bytes()));
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
        std::fs::write(d.path().join(LEAD_REQUEST), r#"{"ask": {"kind": "take", "force": true}, "by": "the menu bar on m1", "at": 5}"#).unwrap();
        assert_eq!(peek_lead(d.path()).map(|r| r.ask), Some(LeadAsk::Take { force: true, downgrade: false }));
        assert_eq!(take_lead(d.path()).map(|r| (r.ask, r.at)), Some((LeadAsk::Take { force: true, downgrade: false }, 5)));
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
    }
}
