//! The owner's controls of the pool's lead (docs/pool.md §6.3–§6.5, §10, §11): their asks reaching
//! the driver, where each stands, and what every view shows of the pool.
//!
//! - **The asks** (crate::control::LeadAsk): from this Mac's menu, `scenic lead` and the map, a file
//!   in the agent's folder (`control::LEAD_REQUEST`); from the build page, its lead's coordinator
//!   (`/work/lead`). The pool's next step takes them (`Controls::take`: the agent's loop's, or
//!   while that's busy its pool's own thread's, crate::agent::pool's `Ticker`) and checks each as
//!   the driver's step would (`Driver::hand_to`, `Driver::takeover`): one refused never reaches it,
//!   and says why; one that passes goes to the next step as the owner's ask, the driver handing
//!   over (on the lead) or passing it on to the lead by mail (on a member, §9: the pool's HTTP API
//!   is planned). Where it stands (`Asked`) is followed from the steps' events and the terms.
//! - **The view** (`View`, in the agent's status): the lead and its term; each member, read by id
//!   from its heartbeat (never listed), with its state and whether the lead can be handed to it,
//!   and why not; what a takeover from this Mac needs; a handover under way; "no lead", and why; the
//!   the ask's state; the last change of lead in words. Made again every
//!   `VIEW_S`, and at once after an ask or a change. A lead whose heartbeat is out of touch while its
//!   coordinator answered this Mac lately (`ANSWERED_S`) is late, not gone (`View::quiet`): no "no
//!   lead", and no takeover unforced.
//! - **The lead's battery low** (§6.4): a lead on battery below `cond::BATTERY_MIN` asks to hand
//!   the lead to a member that can lead (`low_battery`), as the owner's ask would; again every
//!   `BATTERY_AGAIN_S` while it still leads so. The app is otherwise not smart about who leads: the
//!   owner hands the lead over by hand.
//! - **The history** (`notes`): each term made (handed over, taken back, taken over, re-asserted),
//!   stepped down from, and a handover's offer, end, giving up or drop, as `term` events: into the
//!   lead's coordinator's history (the build page's activity, and the NAS's history per writer
//!   through it); a member, without one, appends them to its own file on the NAS and keeps them for
//!   the coordinator of its next process (`NOTES`: it restarts into the lead after a takeover).

use super::pool::{Conds, Heartbeat, Run};
use crate::control::{LeadAsk, LeadRequest};
use crate::coord::history;
use crate::pool::beat::Stage;
use crate::pool::driver::{Ask, Event, Out};
use crate::pool::{reachable, term};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// No handover away from an unreachable lead (`away`) within this of its last such ask (s).
pub const CALM_S: u64 = 1800;
/// A lead on battery below `cond::BATTERY_MIN` asks to hand the lead over again this often while it
/// still leads so (s): its last ask came to nothing (none could take it, or the handover failed).
pub const BATTERY_AGAIN_S: u64 = 300;
/// No lead for this long (by this Mac's clock: since the lead was last heard from, or since this
/// Mac first saw the term unreadable or the lead stood down) is an alert, on every view (s).
pub const ALERT_S: u64 = 3600;
/// The view is made again this often (s): a heartbeat read per member.
const VIEW_S: u64 = 30;
/// The lead's coordinator answered this Mac within this (s): it's there, whatever its heartbeat
/// says, so its heartbeat out of touch isn't "no lead", nor a takeover unforced (its pool's step held
/// up, its writes not landing). A lead's beat goes out of touch at the same age.
pub const ANSWERED_S: u64 = crate::pool::beat::OUT_OF_TOUCH_S;
/// An ask's end is shown this long, then forgotten (s).
const SHOWN_S: u64 = 600;
/// An ask passed on to the lead with nothing come of it in this long is said to have gone unheard (s).
const UNHEARD_S: u64 = 600;
/// The terms' events a member keeps for its next process's coordinator, in the agent's pool folder.
pub const NOTES: &str = "notes.jsonl";
/// What it keeps across its processes, in the agent's pool folder.
const KEPT: &str = "lead.json";

/// Where an ask stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// Not asked of the driver: what it would decide refuses it (`said` says why).
    Refused,
    /// Passed on to the lead (a member's ask to hand the lead over).
    Passed,
    /// Under way: the lead handing over, or this Mac taking over.
    Going,
    /// Done: the lead is where it was asked to be.
    Done,
    /// It came to nothing (`said` says why).
    Failed,
}

/// An ask and where it stands, for the views.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asked {
    pub ask: LeadAsk,
    pub by: String,
    /// When it was asked, and when it came to its state (this Mac's clock).
    pub at: u64,
    pub since: u64,
    pub state: State,
    /// In words.
    pub said: String,
    /// The member it hands the lead to (`give`'s, resolved).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

/// The last change of lead this Mac saw, in words ("No longer leading …").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub at: u64,
    pub said: String,
}

/// What it keeps across its processes (a member that takes the lead restarts into it).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Kept {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asked: Option<Asked>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<Change>,
    /// The last time this Mac, leading on a battery below `cond::BATTERY_MIN`, asked to hand the
    /// lead over (`low_battery`).
    #[serde(skip_serializing_if = "is_zero")]
    pub battery_at: u64,
    /// The member that ask named: the next goes to the one after it (`low_battery`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery_to: Option<String>,
    /// The last time this Mac, leading while members couldn't reach it, asked to hand the lead
    /// away (`away`).
    #[serde(skip_serializing_if = "is_zero")]
    pub away_at: u64,
    /// The term with no lead anyone can reach, as first seen so (its number, since when): for the
    /// alert (`ALERT_S`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_lead: Option<(u64, u64)>,
}

/// The term's lead, as its file says.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeadOf {
    pub term: u64,
    pub member: String,
    pub host: String,
    pub app: String,
    pub since: u64,
    pub how: String,
}

/// A member, as its heartbeat says.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub member: String,
    pub host: String,
    pub app: String,
    /// When it last beat (its clock); 0: no heartbeat read.
    pub beat: u64,
    /// This Mac.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub me: bool,
    /// It leads the current term (the term names it).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub leads: bool,
    /// In words: home on power, on battery (54 %), away, out of touch, clock wrong, stood down, app
    /// too old.
    pub state: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub out_of_touch: bool,
    /// Away from home: its duties would run slowly over Tailscale.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub away: bool,
    /// Whether the lead can be handed to it now (`Driver::hand_to`), and why not.
    pub can_lead: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why_not: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conds: Option<Conds>,
    /// Another member in touch reports it can't reach its coordinator (docs/pool.md §6.8), in
    /// words: "unreachable from m4: connection timed out (…)".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unreached: Option<String>,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

/// What a takeover from this Mac needs (`Driver::takeover`): refused, or the owner's force or
/// downgrade, each with why.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Takeover {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub force: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downgrade: Option<String>,
}

/// A handover under way, as its lead's heartbeat says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handing {
    pub to: String,
    pub host: String,
    pub term: u64,
    pub stage: Stage,
    pub since: u64,
}

/// The pool, as the controls show it (in the agent's status: `PoolView::lead`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct View {
    /// When it was made (this Mac's clock).
    pub at: u64,
    pub term: u64,
    /// The current term's lead (None: the term can't be read whole).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lead: Option<LeadOf>,
    /// This Mac leads it.
    pub leading: bool,
    /// Every member it knows, this Mac's first, then by host.
    pub members: Vec<Member>,
    /// What a takeover from this Mac needs (none while it leads).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub takeover: Option<Takeover>,
    /// No lead anyone can reach, and why: the lead out of touch, stood down, or the term unreadable
    /// ("Take it").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_lead: Option<String>,
    /// The lead's heartbeat is out of touch while its coordinator answered this Mac lately
    /// (`ANSWERED_S`): not "no lead", but its heartbeat late, in words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quiet: Option<String>,
    /// No lead for `ALERT_S` or more: since when, why, and what to do, in words (the menu bar's
    /// icon and the build page show it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alert: Option<String>,
    /// The term is there and reads short to this Mac (not just unreadable for a failing read):
    /// what a member takes past by itself (§6.5).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub short: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handing: Option<Handing>,
    /// Who can't reach whom (docs/pool.md §6.8), each in words with what the owner can do: "m1's
    /// coordinator is unreachable from m4: connection timed out (…)".
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unreached: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asked: Option<Asked>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<Change>,
}

/// The controls' part of the agent's pool, in this process.
#[derive(Debug, Default)]
pub struct Controls {
    pub kept: Kept,
    kept_written: Option<Kept>,
    pub view: Option<View>,
    /// The view is made again at the next loop (an ask, a change).
    stale: bool,
    /// The asks to take up at the next loop besides the folder's: the build page's.
    pub page: Vec<LeadRequest>,
}

impl Controls {
    /// What it kept in the agent's pool folder `dir`.
    pub fn open(dir: &Path) -> Controls {
        let kept: Kept =
            std::fs::read(dir.join(KEPT)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Controls { kept_written: Some(kept.clone()), kept, ..Default::default() }
    }

    fn save(&mut self, dir: &Path) {
        if self.kept_written.as_ref() == Some(&self.kept) {
            return;
        }
        let r = serde_json::to_vec(&self.kept)
            .map_err(anyhow::Error::from)
            .and_then(|b| crate::whole::write(&dir.join(KEPT), &b));
        match r {
            Ok(()) => self.kept_written = Some(self.kept.clone()),
            Err(e) => eprintln!("pool: keeping the lead asks' state: {e:#}"),
        }
    }
}

/// A member named `name` (its id, or its host name, any case): its id; or why there's none.
pub(crate) fn resolve(run: &Run, name: &str) -> Result<String, String> {
    if crate::pool::is_member_id(name) {
        return Ok(name.to_string());
    }
    let found: Vec<String> = match run
        .controls
        .view
        .as_ref()
        .map(|v| {
            v.members.iter().filter(|m| m.host.eq_ignore_ascii_case(name)).map(|m| m.member.clone()).collect::<Vec<_>>()
        })
        .filter(|f| !f.is_empty())
    {
        Some(f) => f,
        None => run
            .side
            .members()
            .iter()
            .filter(|id| run.side.heartbeat(id).is_some_and(|h| h.pool.host.eq_ignore_ascii_case(name)))
            .cloned()
            .collect(),
    };
    match found.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(format!("no member of the pool is called {name}")),
        _ => Err(format!("{} members are called {name}: name it by its member id ({})", found.len(), found.join(", "))),
    }
}

fn host_of(run: &Run, id: &str) -> String {
    run.controls
        .view
        .as_ref()
        .and_then(|v| v.members.iter().find(|m| m.member == id).map(|m| m.host.clone()))
        .or_else(|| run.side.heartbeat(id).map(|h| h.pool.host))
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| id.to_string())
}

/// The owner's asks waiting (this Mac's folder `home`'s, and the build page's), checked as the
/// driver's step would and handed to the next step (`Run::asks`); where each stands noted. The last
/// one asked wins.
pub fn take(run: &mut Run, home: &Path) {
    let mut reqs: Vec<LeadRequest> = std::mem::take(&mut run.controls.page);
    reqs.extend(crate::control::take_lead(home));
    for r in reqs {
        let now = run.side.now();
        let me = run.side.member().id.clone();
        let leads = run.side.driver().leads().is_some();
        let lead_host = run.side.driver().current().lead.as_ref().map(|t| t.host.clone());
        let (state, said, to, ask) = match &r.ask {
            LeadAsk::Give { to } => match resolve(run, to) {
                Err(why) => (State::Refused, why, None, None),
                Ok(id) => {
                    let host = host_of(run, &id);
                    match run.side.hand_to(&id) {
                        Err(why) => {
                            (State::Refused, format!("the lead can't be handed to {host}: {why}"), Some(id), None)
                        }
                        Ok(()) if leads => (
                            State::Going,
                            format!("handing the lead to {host}"),
                            Some(id.clone()),
                            Some(Ask::HandTo(id)),
                        ),
                        Ok(()) => (
                            State::Passed,
                            format!(
                                "asked {} to hand the lead to {}",
                                lead_host.unwrap_or_else(|| "the lead".into()),
                                if id == me { "this Mac".to_string() } else { host }
                            ),
                            Some(id.clone()),
                            Some(Ask::HandTo(id)),
                        ),
                    }
                }
            },
            LeadAsk::Take { force, downgrade } => {
                let t = run.side.takeover();
                match (t.refused, t.force, t.downgrade) {
                    (Some(why), _, _) => (State::Refused, format!("this Mac can't take the lead over: {why}"), None, None),
                    (_, Some(why), _) if !force => (State::Refused, format!("taking the lead over needs the owner's force (scenic lead take --force): {why}"), None, None),
                    _ if !force && !leads && late(run, now) => (State::Refused, format!("taking the lead over needs the owner's force (scenic lead take --force): {}'s coordinator answered this Mac {}, so it's there (its heartbeat is late)", lead_host.clone().unwrap_or_else(|| "the lead".into()), ago(now, run.cx.lead_answered.unwrap_or(now))), None, None),
                    (_, _, Some(why)) if !downgrade => (State::Refused, format!("taking the lead over needs the owner's downgrade (scenic lead take --downgrade): {why}"), None, None),
                    _ => (State::Going, "taking the lead over".to_string(), Some(me), Some(Ask::TakeOver { force: *force, downgrade: *downgrade })),
                }
            }
        };
        eprintln!("pool: {} asked: {said}", r.by);
        run.asks.extend(ask);
        run.controls.kept.asked =
            Some(Asked { ask: r.ask, by: r.by, at: r.at.min(now).max(1), since: now, state, said, to });
        run.controls.stale = true;
    }
}

/// After a step: the ask's state followed, the last change of lead noted, the history's terms
/// written, the view made again when due, and the lead handed away by itself when its members can't
/// reach it (`away`) or its battery is low (`low_battery`). `coord`: this process's coordinator (the lead's), whose history the terms go to.
pub fn after(run: &mut Run, out: &Out, root: &Path, home: &Path, coord: Option<&crate::coord::Coordinator>) {
    let now = run.side.now();
    let dir = home.join("pool");
    let me = run.side.member().id.clone();
    // The history.
    let notes = notes(&out.events, &run.side.member().host, |id| host_of(run, id), now);
    if !notes.is_empty() {
        note(&notes, coord, root, &me, &dir);
        run.controls.stale = true;
    }
    // The last change of lead, in words.
    let cur = run.side.driver().current().lead.clone();
    for e in &out.events {
        if let Event::SteppedDown { term, why } = e {
            // (Members by their host names, not their ids.)
            let why = run.side.members().iter().fold(why.clone(), |w, id| {
                if w.contains(id.as_str()) {
                    w.replace(id.as_str(), &host_of(run, id))
                } else {
                    w
                }
            });
            let next = cur
                .as_ref()
                .filter(|t| t.term > *term)
                .map(|t| format!("; term {} is {}'s ({})", t.term, t.host, t.how))
                .unwrap_or_default();
            run.controls.kept.change =
                Some(Change { at: now, said: format!("No longer leading term {term}: {why}{next}") });
        }
        if let Event::Made { term, how } = e {
            if cur.as_ref().is_some_and(|t| t.term == *term && t.member == me) {
                run.controls.kept.change =
                    Some(Change { at: now, said: format!("This Mac leads term {term} ({how})") });
            }
        }
    }
    // Where the ask stands.
    // (A term naming the member asked for: whether it leads it, as this Mac or its heartbeat says.)
    let to = run.controls.kept.asked.as_ref().and_then(|a| a.to.clone());
    let to_leads = match (&to, &cur) {
        (Some(to), Some(t)) if t.member == *to => {
            if *to == me {
                out.leads == Some(t.term)
            } else {
                run.side.heartbeat(to).is_some_and(|h| h.pool.leads == Some(t.term))
            }
        }
        _ => false,
    };
    if let Some(a) = run.controls.kept.asked.as_mut() {
        let was = a.state;
        follow(a, out, cur.as_ref(), to_leads, &me, now);
        if a.state != was {
            a.since = now;
            eprintln!(
                "pool: the ask {}: {}",
                serde_json::to_value(a.state).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(),
                a.said
            );
        }
        if matches!(a.state, State::Refused | State::Done | State::Failed) && now.saturating_sub(a.since) > SHOWN_S {
            run.controls.kept.asked = None;
        }
    }
    if out.events.iter().any(|e| {
        matches!(e, Event::Made { .. } | Event::SteppedDown { .. } | Event::Handover { .. } | Event::TookUp { .. })
    }) {
        run.controls.stale = true;
    }
    // The view.
    let due = run.controls.stale || run.controls.view.as_ref().is_none_or(|v| now.abs_diff(v.at) >= VIEW_S);
    if due {
        let mut v = view(run);
        alert(&mut v, &mut run.controls.kept, now);
        run.controls.view = Some(v);
        run.controls.stale = false;
    }
    // Leading while members can't reach this Mac: the lead to one they reach (§6.8).
    if let Some(v) = run.controls.view.as_ref().filter(|v| v.leading) {
        let ids: Vec<String> = v.members.iter().map(|m| m.member.clone()).collect();
        let beats: Vec<crate::pool::Beat> =
            ids.iter().filter_map(|id| run.side.heartbeat(id)).filter(|h| !h.shadow).map(|h| h.pool).collect();
        let going = run.controls.kept.asked.as_ref().is_some_and(|a| matches!(a.state, State::Going | State::Passed))
            || v.handing.is_some();
        let unreached = Unreached {
            for_s: reachable::unreachable_for(&beats, &me, now),
            by: reachable::unreached(&beats, &me, now).len(),
            of: beats.iter().filter(|b| b.member != me && b.beat > 0 && !b.out_of_touch(now)).count(),
        };
        let last_change = v.lead.as_ref().map_or(0, |l| l.since);
        if let Some(to) = away(v, unreached, &mut run.controls.kept, now, last_change, going, &|id| {
            reachable::reached(&beats, id, now)
        }) {
            let host = host_of(run, &to);
            let why = v.members.iter().find(|m| m.me).and_then(|m| m.unreached.clone()).unwrap_or_default();
            run.controls.page.push(LeadRequest {
                ask: LeadAsk::Give { to: to.clone() },
                by: format!("the pool by itself (this Mac is {why})"),
                at: now,
            });
            eprintln!("pool: members can't reach this Mac's coordinator ({why}): handing the lead to {host}, which they reach");
        }
    }
    // The lead's battery below the threshold: the lead to a member that can lead.
    if let Some(v) = run.controls.view.clone().filter(|v| v.leading) {
        let going = run.controls.kept.asked.as_ref().is_some_and(|a| matches!(a.state, State::Going | State::Passed))
            || v.handing.is_some();
        if let Some(to) = low_battery(&v, &mut run.controls.kept, now, going) {
            let host = host_of(run, &to);
            let pct = v.members.iter().find(|m| m.me).and_then(|m| m.conds).and_then(|c| c.battery).unwrap_or(0);
            let why = format!("its battery at {pct} %, below {} %", crate::agent::cond::BATTERY_MIN);
            run.controls.page.push(LeadRequest {
                ask: LeadAsk::Give { to: to.clone() },
                by: format!("the pool by itself ({why})"),
                at: now,
            });
            eprintln!("pool: this Mac leads with {why}: handing the lead to {host}");
        }
    }
    run.controls.save(&dir);
}

/// Follows ask `a` from a step's events and the current term `cur`.
fn follow(a: &mut Asked, out: &Out, cur: Option<&term::Term>, to_leads: bool, me: &str, now: u64) {
    if !matches!(a.state, State::Going | State::Passed) {
        return;
    }
    let to = a.to.clone().unwrap_or_default();
    match &a.ask {
        LeadAsk::Give { .. } => {
            for e in &out.events {
                match e {
                    Event::Handover { to: t, what, .. } if *t == to => match *what {
                        "passed" => a.said = format!("{} — passed: waiting for it to take up", a.said.split(" — ").next().unwrap_or_default()),
                        "over" => (a.state, a.said) = (State::Done, format!("handed over: the term names {}", cur.map_or(to.clone(), |t| t.host.clone()))),
                        "given up" => (a.state, a.said) = (State::Failed, "the handover was given up (the other Mac didn't answer, or the lead couldn't settle in time)".to_string()),
                        "taken back" => (a.state, a.said) = (State::Failed, "the other Mac didn't take up in time: the lead took the build back".to_string()),
                        "dropped" => (a.state, a.said) = (State::Failed, "the handover was dropped".to_string()),
                        _ => {}
                    },
                    Event::Waits { what: "hand the lead over", why } => (a.state, a.said) = (State::Failed, format!("the lead refused it: {why}")),
                    _ => {}
                }
            }
            // (Done once the member asked for leads the current term, wherever this Mac heard it.)
            if matches!(a.state, State::Going | State::Passed) && to_leads {
                (a.state, a.said) = (
                    State::Done,
                    format!(
                        "{} leads term {}",
                        cur.map(|t| t.host.clone()).unwrap_or_default(),
                        cur.map_or(0, |t| t.term)
                    ),
                );
            } else if a.state == State::Passed && now.saturating_sub(a.at) > UNHEARD_S {
                (a.state, a.said) = (State::Failed, "nothing came of it in ten minutes: the lead didn't hear it, or didn't hand over (its status says why)".to_string());
            }
        }
        LeadAsk::Take { .. } => {
            for e in &out.events {
                if let Event::Waits { what: "take over", why } | Event::Failed { what: "take over", why } = e {
                    (a.state, a.said) = (State::Failed, format!("couldn't take over: {why}"));
                }
            }
            if a.state == State::Going && out.leads.is_some() && cur.is_some_and(|t| t.member == me) {
                (a.state, a.said) = (State::Done, format!("this Mac leads term {}", out.leads.unwrap_or_default()));
            }
        }
    }
}

/// A member's state in words (§11), at the reader's `now`, against the term's app `app`.
pub fn state_of(h: &Heartbeat, now: u64, app: Option<&str>) -> String {
    let b = &h.pool;
    if b.beat == 0 {
        return "no heartbeat".into();
    }
    if b.out_of_touch(now) {
        return "out of touch".into();
    }
    if b.clock_wrong(now) {
        return "clock wrong".into();
    }
    if b.stood_down.is_some() {
        return "stood down".into();
    }
    if app.is_some_and(|a| !term::app_at_least(&b.app, a)) {
        return "app too old".into();
    }
    match h.conds {
        None => "conditions unknown (an app before the controls)".into(),
        Some(c) if c.held => "paused".into(),
        Some(c) if !c.home => "away".into(),
        Some(c) if !c.ac => c.battery.map_or("on battery".to_string(), |p| format!("on battery ({p} %)")),
        Some(_) => "home on power".into(),
    }
}

/// Whether the current term's lead (not this Mac) is out of touch by its heartbeat, not stood down,
/// while its coordinator answered this Mac lately (`lead_answered`).
fn late(run: &Run, now: u64) -> bool {
    let Some(t) = run.side.driver().current().lead.clone() else { return false };
    lead_answered(run, now).is_some()
        && t.member != run.side.member().id
        && run.side.heartbeat(&t.member).is_some_and(|h| h.pool.out_of_touch(now) && h.pool.stood_down != Some(t.term))
}

/// When the lead's coordinator last answered this Mac, if within `ANSWERED_S` of `now`.
fn lead_answered(run: &Run, now: u64) -> Option<u64> {
    run.cx.lead_answered.filter(|a| now.saturating_sub(*a) < ANSWERED_S)
}

/// The view, made now.
fn view(run: &Run) -> View {
    let side = &run.side;
    let now = side.now();
    let me = side.member().id.clone();
    let cur = side.driver().current().clone();
    let leading = side.driver().leads().is_some();
    let lead = cur.lead.as_ref().map(|t| LeadOf {
        term: t.term,
        member: t.member.clone(),
        host: t.host.clone(),
        app: t.app.clone(),
        since: t.since,
        how: t.how.clone(),
    });
    let mut ids: Vec<String> = side.members().iter().cloned().collect();
    if let Some(t) = &cur.lead {
        if !ids.contains(&t.member) {
            ids.push(t.member.clone());
        }
    }
    let pool_beats: Vec<crate::pool::Beat> =
        ids.iter().filter_map(|id| side.heartbeat(id)).filter(|h| !h.shadow).map(|h| h.pool).collect();
    let mut beats: Vec<(String, Heartbeat)> = Vec::new();
    let mut members: Vec<Member> = Vec::new();
    // (The app a member needs: the term's, and once a readers' switch is recorded, the one it was
    // recorded on, whichever is newer: an older one is "app too old".)
    // (Read once for the view: every member's handover and state is judged by it.)
    let regions = side.regions_app();
    let need = crate::inputs::regions::read::needed_app(
        cur.lead.as_ref().map(|t| t.app.as_str()),
        regions.as_ref().ok().and_then(|r| r.as_deref()),
    );
    for id in ids {
        let h = side.heartbeat(&id).unwrap_or_default();
        // (Not a shadow run's heartbeat, nor one of another time's member that never beat.)
        if h.shadow {
            continue;
        }
        let is_lead = cur.lead.as_ref().is_some_and(|t| t.member == id);
        let (can, why) = if is_lead {
            (false, Some("it leads".to_string()))
        } else {
            match side.hand_to_with(&id, &regions) {
                Ok(()) => (true, None),
                Err(w) => (false, Some(w)),
            }
        };
        let host = if h.pool.host.is_empty() {
            if id == me {
                side.member().host.clone()
            } else {
                id.clone()
            }
        } else {
            h.pool.host.clone()
        };
        let unreached = reachable::why_unreachable(&pool_beats, &id, &host, now);
        members.push(Member {
            member: id.clone(),
            host,
            app: h.pool.app.clone(),
            beat: h.pool.beat,
            me: id == me,
            leads: is_lead,
            state: match &regions {
                Err(e) => format!("the readers' switches' app unknown ({e})"),
                Ok(_) => state_of(&h, now, need.as_deref()),
            },
            out_of_touch: h.pool.beat == 0 || h.pool.out_of_touch(now),
            away: h.conds.is_some_and(|c| !c.home),
            can_lead: can,
            why_not: why,
            conds: h.conds,
            unreached,
        });
        beats.push((id, h));
    }
    members.sort_by(|a, b| b.me.cmp(&a.me).then(b.leads.cmp(&a.leads)).then(a.host.cmp(&b.host)));
    let lead_beat =
        cur.lead.as_ref().and_then(|t| beats.iter().find(|(id, _)| *id == t.member)).map(|(_, h)| h.clone());
    // (Its coordinator answered lately: there, its heartbeat late.)
    let answered = lead_answered(run, now);
    let quiet = match (&cur.lead, &lead_beat, answered) {
        (Some(t), Some(h), Some(a))
            if !leading
                && t.member != me
                && h.pool.beat > 0
                && h.pool.out_of_touch(now)
                && h.pool.stood_down != Some(t.term) =>
        {
            Some(format!(
                "{}'s heartbeat is late (last {}), but its coordinator answered this Mac {}: it's there",
                t.host,
                ago(now, h.pool.beat),
                ago(now, a)
            ))
        }
        _ => None,
    };
    let no_lead = match (&cur.lead, &lead_beat) {
        _ if cur.term == 0 => None,
        (None, _) => Some(format!("term {} can't be read whole", cur.term)),
        (Some(t), _) if leading || t.member == me => None,
        (Some(t), None) => Some(format!("no heartbeat from {}", t.host)),
        (Some(t), Some(h)) if h.pool.beat == 0 => Some(format!("no heartbeat from {}", t.host)),
        (Some(t), Some(h)) if h.pool.out_of_touch(now) && quiet.is_none() => {
            Some(format!("{} is out of touch: last heard from {}", t.host, ago(now, h.pool.beat)))
        }
        (Some(t), Some(h)) if h.pool.stood_down == Some(t.term) => {
            Some(format!("{} stood down from term {} (its app is older than the term's)", t.host, t.term))
        }
        _ => None,
    };
    let handing = lead_beat.as_ref().and_then(|h| h.pool.handing_to.as_ref()).map(|ht| Handing {
        to: ht.to.clone(),
        host: members.iter().find(|m| m.member == ht.to).map_or(ht.to.clone(), |m| m.host.clone()),
        term: ht.term,
        stage: ht.stage,
        since: ht.since,
    });
    let takeover = (!leading).then(|| {
        let t = side.takeover();
        Takeover { refused: t.refused, force: t.force, downgrade: t.downgrade }
    });
    let unreached = members
        .iter()
        .flat_map(|m| reachable::unreached(&pool_beats, &m.member, now).into_iter().map(move |(b, r)| (m, b, r)))
        .map(|(m, b, r)| {
            let by = if b.host.is_empty() { b.member.clone() } else { b.host.clone() };
            let what = if r.what == "coordinator" { "coordinator" } else { "broker" };
            let lost = if m.leads && r.what == "coordinator" {
                format!(": {by} gets no work while {} leads", m.host)
            } else {
                String::new()
            };
            format!(
                "{}'s {what} is unreachable from {by} for {}: {}{lost}",
                m.host,
                span(b.beat.saturating_sub(r.since)),
                reachable::words(r, &m.host)
            )
        })
        .collect();
    View {
        at: now,
        term: cur.term,
        lead,
        leading,
        members,
        takeover,
        no_lead,
        quiet,
        alert: None,
        short: side.driver().unread_since().is_some(),
        handing,
        unreached,
        asked: run.controls.kept.asked.clone(),
        change: run.controls.kept.change.clone(),
    }
}

/// The alert (`View::alert`): no lead for `ALERT_S` or more, since the lead was last heard from or
/// since this Mac first saw no lead in this term (`kept.no_lead`), whichever is earlier; with why,
/// and the command that takes it over (what a takeover from this Mac needs, or from another Mac
/// when this one can't).
pub fn alert(v: &mut View, kept: &mut Kept, now: u64) {
    // (A lead late, its coordinator answering: counted all the same, its takeover the owner's
    // forced one.)
    let late = v.no_lead.is_none() && v.quiet.is_some();
    let why = v.no_lead.clone().or_else(|| {
        v.lead
            .as_ref()
            .filter(|_| late)
            .map(|l| format!("{}'s heartbeat is out of touch, though its coordinator still answers", l.host))
    });
    let Some(why) = why else {
        kept.no_lead = None;
        v.alert = None;
        return;
    };
    let seen = match kept.no_lead {
        Some((e, since)) if e == v.term => since,
        _ => {
            kept.no_lead = Some((v.term, now));
            now
        }
    };
    let heard = v
        .lead
        .as_ref()
        .and_then(|l| v.members.iter().find(|m| m.member == l.member))
        .map(|m| m.beat)
        .filter(|&b| b > 0);
    let since = heard.map_or(seen, |b| b.min(seen));
    if now.saturating_sub(since) < ALERT_S {
        v.alert = None;
        return;
    }
    let what = match &v.takeover {
        Some(Takeover { refused: None, .. }) if late => "if it's stuck, run `scenic lead take --force` on this Mac (or \"Take Over the Build…\" in its menu) to lead from the records on the NAS; nothing built is lost".to_string(),
        Some(t @ Takeover { refused: None, .. }) => {
            let flags: String = [t.force.as_ref().map(|_| " --force"), t.downgrade.as_ref().map(|_| " --downgrade")].into_iter().flatten().collect();
            format!("run `scenic lead take{flags}` on this Mac (or \"Take Over the Build…\" in its menu) to lead from the records on the NAS; nothing built is lost")
        }
        Some(Takeover { refused: Some(w), .. }) => format!("this Mac can't take it over ({w}): run `scenic lead take --force` on another Mac in the pool"),
        None => "run `scenic lead take --force` on a Mac in the pool".to_string(),
    };
    // (A term unreadable is taken past by itself, a lead silent or stood down taken over (§6.5):
    // no lead still, none could.)
    let auto = if v.lead.is_none() && v.short {
        "; no Mac took it past by itself (none can lead now, or the app rule refuses)"
    } else if v.lead.is_some() && !late {
        "; no Mac took it over by itself (none can lead now, or the app rule refuses)"
    } else {
        ""
    };
    v.alert = Some(format!(
        "No lead for {}: {why}{auto}. Nothing builds until a Mac leads: {what}.",
        span(now.saturating_sub(since))
    ));
}

/// How the members in touch report this Mac's coordinator: the longest any has failed to reach it
/// (s, by its clock), how many fail to, and of how many (the members in touch but this Mac).
#[derive(Clone, Copy, Debug, Default)]
pub struct Unreached {
    pub for_s: Option<u64>,
    pub by: usize,
    pub of: usize,
}

/// The lead hands itself away (docs/pool.md §6.8) when at least half the members in touch, one at
/// least, report they can't reach its coordinator, one of them for `reachable::AWAY_S` or more;
/// nothing under way (`busy`: a handover, an ask); its term `AWAY_S` old or more (`last_change`)
/// and its last such ask `CALM_S` ago or more (`kept.away_at`). To the member that can lead
/// (`Member::can_lead`: which no member reports unreachable), that another member reached lately
/// (`reached`), home on power first, then by host; none: nothing. The ask's time is kept.
pub fn away(
    v: &View,
    u: Unreached,
    kept: &mut Kept,
    now: u64,
    last_change: u64,
    busy: bool,
    reached: &dyn Fn(&str) -> bool,
) -> Option<String> {
    let long = u.for_s.is_some_and(|s| s >= reachable::AWAY_S);
    let enough = u.by >= 1 && u.by * 2 >= u.of;
    let calm = now.saturating_sub(last_change) >= reachable::AWAY_S && now.saturating_sub(kept.away_at) >= CALM_S;
    if !v.leading || !long || !enough || busy || !calm {
        return None;
    }
    let to = v
        .members
        .iter()
        .filter(|m| !m.me && !m.leads && m.can_lead && reached(&m.member))
        .min_by_key(|m| (!m.conds.is_some_and(|c| c.home && c.ac), m.host.clone()))?;
    kept.away_at = now;
    Some(to.member.clone())
}

/// A span of seconds in words: "75 min", "5 h", "3 days".
fn span(s: u64) -> String {
    match s {
        0..=5399 => format!("{} min", s / 60),
        5400..=129_599 => format!("{} h", s / 3600),
        _ => format!("{} days", s / 86400),
    }
}

/// What the owner does about a coordinator members can't reach (docs/pool.md §6.8).
pub const OWNER_STEPS: &str = "On the Mac named: check it is awake, on the network and its agent running; then allow Scenic in System Settings › Network › Firewall (Options…), or turn the firewall off. Until then the lead stays off it";

/// The lead's battery low (docs/pool.md §6.4): this Mac leads on battery below
/// `cond::BATTERY_MIN` (its heartbeat's conditions), nothing under way (`busy`: a handover, an
/// ask), its last such ask `BATTERY_AGAIN_S` ago or more (`kept.battery_at`). To a member that
/// can lead (`Member::can_lead`), in host order, round-robin: the one after the member last asked
/// (`kept.battery_to`), so one that never answered isn't asked again while another could take it;
/// none: nothing, and it keeps leading. The ask's time and target are kept.
pub fn low_battery(v: &View, kept: &mut Kept, now: u64, busy: bool) -> Option<String> {
    let me = v.members.iter().find(|m| m.me)?.conds?;
    let low = !me.ac && me.battery.is_some_and(|b| b < crate::agent::cond::BATTERY_MIN);
    if !v.leading || !low || busy || now.saturating_sub(kept.battery_at) < BATTERY_AGAIN_S {
        return None;
    }
    let can: Vec<&Member> = v.members.iter().filter(|m| !m.me && !m.leads && m.can_lead).collect();
    let after = kept.battery_to.as_ref().and_then(|t| can.iter().position(|m| m.member == *t)).map_or(0, |i| i + 1);
    let to = can.get(after % can.len().max(1))?.member.clone();
    kept.battery_at = now;
    kept.battery_to = Some(to.clone());
    Some(to)
}

/// The step's events of the terms, as the history notes them (kind `term`, `worker` this Mac's
/// host): each term made (its `how`: handed over, taken back, taken over, re-asserted), stepped down
/// from, and a handover's offer, end, giving up and drop. `host`: a member's host by its id.
pub fn notes(events: &[Event], me: &str, host: impl Fn(&str) -> String, now: u64) -> Vec<history::Event> {
    let mut out = Vec::new();
    for e in events {
        let note = match e {
            Event::Made { term, how } => format!("term {term}: {how}"),
            Event::SteppedDown { term, why } => match why.strip_prefix("handed over to ") {
                Some(to) => format!("stepped down from term {term}: handed over to {}", host(to)),
                None => format!("stepped down from term {term}: {why}"),
            },
            Event::Handover { term, to, what } if matches!(*what, "offered" | "over" | "given up" | "dropped") => {
                format!("its handover of term {term} to {}: {what}", host(to))
            }
            _ => continue,
        };
        out.push(history::Event {
            t: now,
            kind: "term".into(),
            worker: Some(me.to_string()),
            note,
            ..Default::default()
        });
    }
    out
}

/// The terms' events noted: appended to this member's history on the NAS (`root`) at once (a lead
/// stepping down appends its coordinator's history no more: crate::agent's `pool_lead_files`
/// leaves them out); and into this process's coordinator's history, when it has one, else kept in
/// its pool folder `dir` for its next process's coordinator (`replay`).
fn note(events: &[history::Event], coord: Option<&crate::coord::Coordinator>, root: &Path, member: &str, dir: &Path) {
    if let Err(e) = super::pool::append_history(root, member, events) {
        eprintln!("pool: the terms in the history on the NAS: {e:#}");
    }
    if let Some(c) = coord {
        for e in events {
            c.note(e.clone());
        }
        return;
    }
    let mut b = Vec::new();
    for e in events {
        b.extend(serde_json::to_vec(e).unwrap_or_default());
        b.push(b'\n');
    }
    use std::io::Write;
    let r = std::fs::create_dir_all(dir)
        .and_then(|_| std::fs::OpenOptions::new().create(true).append(true).open(dir.join(NOTES)))
        .and_then(|mut f| f.write_all(&b));
    if let Err(e) = r {
        eprintln!("pool: keeping the terms' events: {e}");
    }
}

/// The terms' events an earlier process of this member kept (`note`), into this process's
/// coordinator's history (the build page's activity; on the NAS already); their file removed.
pub fn replay(dir: &Path, coord: &crate::coord::Coordinator) {
    let p: PathBuf = dir.join(NOTES);
    let Ok(text) = std::fs::read_to_string(&p) else { return };
    for l in text.lines() {
        if let Ok(e) = serde_json::from_str::<history::Event>(l) {
            coord.note(history::Event { seq: 0, ..e });
        }
    }
    std::fs::remove_file(&p).ok();
}

/// This Mac's own agent's status (its folder `home`): the lead's `status.json` or a member's
/// `helper.json`, the fresher.
///
/// Its pool part as the pool's own thread last wrote it (crate::agent::pool's `VIEW`), when that's
/// fresher: the status is written at the end of the agent's loop, which a long step holds up, while
/// the pool is stepped every 20 s or so whatever the loop does.
pub fn own_status(home: &Path) -> Option<super::Status> {
    let mut st = own_status_plain(home)?;
    if let Some(v) = super::pool::read_view(home).filter(|v| v.beat > st.beat) {
        st.pool = Some(v.pool);
    }
    Some(st)
}

/// This Mac's agent's pool part, the fresher of its status's and its pool's own (`own_status`), if
/// written within `fresh_s` of `now`, with when it was.
pub fn own_pool(home: &Path, now: u64, fresh_s: u64) -> Option<(super::PoolView, u64)> {
    let from_status = own_status_plain(home).and_then(|s| Some((s.pool?, s.beat)));
    let from_view = super::pool::read_view(home).map(|v| (v.pool, v.beat));
    [from_status, from_view]
        .into_iter()
        .flatten()
        .max_by_key(|(_, at)| *at)
        .filter(|(_, at)| now.saturating_sub(*at) < fresh_s)
}

fn own_status_plain(home: &Path) -> Option<super::Status> {
    ["status.json", "helper.json"]
        .iter()
        .filter_map(|f| std::fs::read(home.join(f)).ok())
        .filter_map(|b| serde_json::from_slice::<super::Status>(&b).ok())
        .max_by_key(|s| s.beat)
}

/// How long ago `t` was, at `now`, in words.
fn ago(now: u64, t: u64) -> String {
    match now.saturating_sub(t) {
        s @ 0..=89 => format!("{s} s ago"),
        s @ 90..=5399 => format!("{} min ago", s / 60),
        s @ 5400..=129_599 => format!("{} h ago", s / 3600),
        s => format!("{} days ago", s / 86400),
    }
}

/// The view in words, a line each, for `scenic lead` and `scenic status` (at the reader's `now`).
pub fn said(v: &View, now: u64) -> Vec<String> {
    let mut out = Vec::new();
    match (&v.lead, &v.no_lead) {
        (_, Some(why)) => {
            out.push(v.alert.clone().unwrap_or_else(|| format!("No lead: {why} (`scenic lead take` takes it over)")))
        }
        (Some(l), None) => out.push(format!(
            "The lead: {}{} — term {}, since {} ({})",
            l.host,
            if v.leading { " (this Mac)" } else { "" },
            l.term,
            ago(now, l.since),
            l.how
        )),
        (None, None) => out.push("No terms yet".into()),
    }
    if let Some(q) = &v.quiet {
        out.push(q.clone());
    }
    if let Some(h) = &v.handing {
        let stage = serde_json::to_value(h.stage).ok().and_then(|s| s.as_str().map(str::to_string)).unwrap_or_default();
        out.push(format!("Handing over to {}: {stage}, {}", h.host, ago(now, h.since)));
    }
    for m in &v.members {
        let mut tags: Vec<&str> = Vec::new();
        if m.me {
            tags.push("this Mac");
        }
        if m.leads {
            tags.push("leads");
        }
        let tags = if tags.is_empty() { String::new() } else { format!(" ({})", tags.join(", ")) };
        let heard = if m.out_of_touch && m.beat > 0 {
            format!(", last heard from {}", ago(now, m.beat))
        } else {
            String::new()
        };
        let can = match (&m.why_not, m.leads) {
            (_, true) => String::new(),
            (None, _) => " — can lead".to_string(),
            (Some(w), _) => format!(" — can't lead: {w}"),
        };
        out.push(format!(
            "  {}{tags}: {}{heard}, app {}{can}",
            m.host,
            m.state,
            if m.app.is_empty() { "?" } else { &m.app }
        ));
    }
    for u in &v.unreached {
        out.push(u.clone());
    }
    if !v.unreached.is_empty() {
        out.push(OWNER_STEPS.to_string());
    }
    if let Some(t) = &v.takeover {
        if v.no_lead.is_some() || t.refused.is_none() {
            let needs: Vec<String> = [
                t.force.as_ref().map(|w| format!("--force ({w})")),
                t.downgrade.as_ref().map(|w| format!("--downgrade ({w})")),
            ]
            .into_iter()
            .flatten()
            .collect();
            out.push(match (&t.refused, needs.is_empty()) {
                (Some(w), _) => format!("Taking over from this Mac: not now ({w})"),
                (None, true) => "Taking over from this Mac: `scenic lead take`".into(),
                (None, false) => format!("Taking over from this Mac needs {}", needs.join(" and ")),
            });
        }
    }
    if let Some(a) = &v.asked {
        let state = serde_json::to_value(a.state).ok().and_then(|s| s.as_str().map(str::to_string)).unwrap_or_default();
        out.push(format!("The last ask ({}, {}): {state} — {}", a.by, ago(now, a.at), a.said));
    }
    if let Some(c) = &v.change {
        out.push(format!("{} ({})", c.said, ago(now, c.at)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::beat::Beat;

    fn hb(host: &str, beat: u64, conds: Option<Conds>) -> Heartbeat {
        Heartbeat {
            pool: Beat {
                member: format!("m-{host}"),
                host: host.into(),
                app: "development".into(),
                beat,
                leads: None,
                ..Default::default()
            },
            conds,
            ..Default::default()
        }
    }

    const HOME: Conds = Conds { home: true, ac: true, battery: Some(90), able: true, held: false, target_mb: 0 };

    #[test]
    fn the_alert_counts_from_the_first_sight_of_no_lead_or_the_leads_last_beat() {
        let mut kept = Kept::default();
        // Term 55 unreadable (the 12 Oct case): seen at 1000, an alert from an hour on, saying
        // to force.
        let mut v = View {
            term: 55,
            short: true,
            no_lead: Some("term 55 can't be read whole".into()),
            takeover: Some(Takeover { force: Some("term 55 can't be read whole".into()), ..Default::default() }),
            ..Default::default()
        };
        alert(&mut v, &mut kept, 1000);
        assert_eq!((v.alert.as_deref(), kept.no_lead), (None, Some((55, 1000))));
        alert(&mut v, &mut kept, 1000 + ALERT_S - 1);
        assert_eq!(v.alert, None);
        alert(&mut v, &mut kept, 1000 + 3 * 3600);
        assert_eq!(v.alert.as_deref(), Some("No lead for 3 h: term 55 can't be read whole; no Mac took it past by itself (none can lead now, or the app rule refuses). Nothing builds until a Mac leads: run `scenic lead take --force` on this Mac (or \"Take Over the Build…\" in its menu) to lead from the records on the NAS; nothing built is lost."));
        // A later term with no lead counts again from its first sight; this Mac unable, another's.
        let mut v = View {
            term: 56,
            no_lead: Some("term 56 can't be read whole".into()),
            takeover: Some(Takeover {
                refused: Some("term 56, which this Mac made, isn't written whole yet".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        alert(&mut v, &mut kept, 20_000);
        assert_eq!((v.alert.as_deref(), kept.no_lead), (None, Some((56, 20_000))));
        alert(&mut v, &mut kept, 20_000 + ALERT_S);
        assert!(
            v.alert.as_deref().is_some_and(|w| w.contains("this Mac can't take it over (term 56, which this Mac made")
                && w.contains("`scenic lead take --force` on another Mac")),
            "{v:?}"
        );
        // The lead out of touch: from its last beat, though first seen just now.
        let lead = LeadOf { term: 56, member: "m-a".into(), host: "Mac-mini".into(), ..Default::default() };
        let mut v = View {
            term: 56,
            lead: Some(lead),
            members: vec![Member { member: "m-a".into(), beat: 30_000, ..Default::default() }],
            no_lead: Some("Mac-mini is out of touch".into()),
            takeover: Some(Takeover::default()),
            ..Default::default()
        };
        kept.no_lead = None;
        alert(&mut v, &mut kept, 30_000 + ALERT_S + 120);
        assert!(
            v.alert.as_deref().is_some_and(|w| w.starts_with("No lead for 62 min: Mac-mini is out of touch; no Mac took it over by itself (none can lead now, or the app rule refuses). ")
                && w.contains("`scenic lead take` on this Mac")),
            "{v:?}"
        );
        // A lead again: none, and forgotten.
        let mut v = View { term: 57, ..Default::default() };
        alert(&mut v, &mut kept, 40_000);
        assert_eq!((v.alert, kept.no_lead), (None, None));
    }

    #[test]
    fn a_members_state_in_words() {
        let now = 10_000;
        assert_eq!(state_of(&hb("a", now - 5, Some(HOME)), now, Some("development")), "home on power");
        assert_eq!(
            state_of(&hb("a", now - 5, Some(Conds { ac: false, battery: Some(54), ..HOME })), now, None),
            "on battery (54 %)"
        );
        assert_eq!(state_of(&hb("a", now - 5, Some(Conds { home: false, ..HOME })), now, None), "away");
        assert_eq!(state_of(&hb("a", now - 601, Some(HOME)), now, None), "out of touch");
        assert_eq!(state_of(&hb("a", now + 61, Some(HOME)), now, None), "clock wrong");
        assert_eq!(state_of(&hb("a", now, Some(HOME)), now, Some("20991231-2359-1234567")), "app too old");
        assert!(state_of(&hb("a", now, None), now, None).starts_with("conditions unknown"));
        assert_eq!(state_of(&Heartbeat::default(), now, None), "no heartbeat");
    }

    fn member(host: &str, conds: Conds, can: bool) -> Member {
        Member {
            member: format!("m-{host}"),
            host: host.into(),
            can_lead: can,
            conds: Some(conds),
            state: String::new(),
            ..Default::default()
        }
    }

    #[test]
    fn the_lead_hands_over_only_below_the_battery_threshold() {
        let lead = |c: Conds| Member { me: true, leads: true, ..member("mini", c, false) };
        let v = |c: Conds| View {
            leading: true,
            members: vec![lead(c), member("air", HOME, true), member("zed", HOME, false)],
            ..Default::default()
        };
        let t0 = 1_000_000;
        let low = Conds { ac: false, battery: Some(crate::agent::cond::BATTERY_MIN - 1), ..HOME };
        // Home on power, on battery at or above the threshold, away from home with the NAS: nothing.
        for c in [
            HOME,
            Conds { ac: false, battery: Some(crate::agent::cond::BATTERY_MIN), ..HOME },
            Conds { ac: false, battery: Some(54), ..HOME },
            Conds { home: false, ..HOME },
            Conds { home: false, ac: false, battery: Some(80), ..HOME },
        ] {
            assert_eq!(low_battery(&v(c), &mut Kept::default(), t0, false), None, "{c:?}");
        }
        // Below it: to the member that can lead.
        let mut k = Kept::default();
        assert_eq!(low_battery(&v(low), &mut k, t0, false), Some("m-air".into()));
        assert_eq!(k.battery_at, t0);
        // Asked again only after a while; not while a handover or an ask is under way; not as a member.
        assert_eq!(low_battery(&v(low), &mut k, t0 + BATTERY_AGAIN_S - 1, false), None);
        assert_eq!(low_battery(&v(low), &mut k, t0 + BATTERY_AGAIN_S, true), None);
        assert_eq!(low_battery(&v(low), &mut k, t0 + BATTERY_AGAIN_S, false), Some("m-air".into()), "the only one");
        // Two that can lead: air never answered, so the next ask goes to pro, then round to air.
        let mut two = v(low);
        two.members.push(member("pro", HOME, true));
        let mut k = Kept::default();
        assert_eq!(low_battery(&two, &mut k, t0, false), Some("m-air".into()));
        assert_eq!(low_battery(&two, &mut k, t0 + BATTERY_AGAIN_S, false), Some("m-pro".into()));
        assert_eq!(low_battery(&two, &mut k, t0 + 2 * BATTERY_AGAIN_S, false), Some("m-air".into()));
        // The one last asked gone from those that can: the first again.
        let mut k = Kept { battery_to: Some("m-gone".into()), ..Default::default() };
        assert_eq!(low_battery(&two, &mut k, t0, false), Some("m-air".into()));
        assert_eq!(low_battery(&View { leading: false, ..v(low) }, &mut Kept::default(), t0, false), None);
        // None can lead: nothing.
        let mut none = v(low);
        none.members.retain(|m| m.me || !m.can_lead);
        assert_eq!(low_battery(&none, &mut Kept::default(), t0, false), None);
    }

    #[test]
    fn a_lead_hands_itself_away_only_reported_long_by_half_calm_and_to_a_member_reached() {
        use crate::pool::reachable::AWAY_S;
        let m = |host: &str, me: bool, can: bool, conds: Conds| Member {
            member: format!("m-{host}"),
            host: host.into(),
            me,
            leads: me,
            can_lead: can,
            conds: Some(conds),
            ..Default::default()
        };
        let home = Conds { home: true, ac: true, battery: None, able: true, held: false, target_mb: 0 };
        let batt = Conds { ac: false, battery: Some(80), ..home };
        let v = View {
            leading: true,
            members: vec![
                m("m1", true, false, home),
                m("air", false, true, batt),
                m("mini", false, true, home),
                m("old", false, false, home),
            ],
            ..Default::default()
        };
        let t0 = 1_000_000;
        let u = |for_s, by, of| Unreached { for_s: Some(for_s), by, of };
        let all = |_: &str| true;
        let mut k = Kept::default();
        // Reported too briefly, by too few, the term too young, something under way: nothing.
        assert_eq!(away(&v, u(AWAY_S - 1, 1, 1), &mut k, t0, 0, false, &all), None);
        assert_eq!(away(&v, u(AWAY_S, 1, 3), &mut k, t0, 0, false, &all), None);
        assert_eq!(away(&v, u(AWAY_S, 1, 1), &mut k, t0, t0 - AWAY_S + 1, false, &all), None);
        assert_eq!(away(&v, u(AWAY_S, 1, 1), &mut k, t0, 0, true, &all), None);
        assert_eq!(away(&v, Unreached::default(), &mut k, t0, 0, false, &all), None);
        // No member another reached: nothing.
        assert_eq!(away(&v, u(AWAY_S, 1, 1), &mut k, t0, 0, false, &|_| false), None);
        // Half of them, long enough: to the one home on power that can lead, then not again for CALM_S.
        assert_eq!(away(&v, u(AWAY_S, 1, 2), &mut k, t0, 0, false, &all), Some("m-mini".into()));
        assert_eq!(away(&v, u(AWAY_S, 1, 2), &mut k, t0 + CALM_S - 1, 0, false, &all), None);
        assert_eq!(away(&v, u(AWAY_S, 1, 2), &mut k, t0 + CALM_S, 0, false, &|id| id == "m-air"), Some("m-air".into()));
        // Not leading: never.
        let mut k = Kept::default();
        assert_eq!(away(&View { leading: false, ..v.clone() }, u(AWAY_S, 1, 1), &mut k, t0, 0, false, &all), None);
    }

    #[test]
    fn the_terms_events_in_the_history() {
        let events = [
            Event::Made { term: 4, how: "handed over by mini".into() },
            Event::Handover { term: 3, to: "m-air".into(), what: "settling" },
            Event::Handover { term: 3, to: "m-air".into(), what: "over" },
            Event::SteppedDown { term: 3, why: "handed over to m-air".into() },
            Event::Merged { applied: vec![], overtaken: 0, refused: 0, listed: false, done: vec![] },
        ];
        let n = notes(&events, "mini", |id| id.trim_start_matches("m-").to_string(), 5);
        let said: Vec<&str> = n.iter().map(|e| e.note.as_str()).collect();
        assert_eq!(
            said,
            [
                "term 4: handed over by mini",
                "its handover of term 3 to air: over",
                "stepped down from term 3: handed over to air"
            ]
        );
        assert!(n.iter().all(|e| e.kind == "term" && e.worker.as_deref() == Some("mini") && e.t == 5));
    }

    /// Two members of a pool in one process, A (this Mac, the writer: term 1 is its) and B, each
    /// with its folder, stepped as the agent steps them (`go`).
    mod runs {
        use super::super::*;
        use crate::agent::pool::{Gates, Role, Run, SharedNas, Side};
        use crate::pool::nas::Share;
        use std::sync::Arc;

        const HOME: Conds = Conds { home: true, ac: true, battery: None, able: true, held: false, target_mb: 0 };

        pub(super) struct Mac {
            pub conds: Conds,
            pub run: Run,
            pub home: PathBuf,
            pub root: PathBuf,
        }

        pub(super) fn pool(d: &Path) -> (Mac, Mac) {
            let r = d.join("nas");
            std::fs::create_dir_all(r.join("state/build")).unwrap();
            std::fs::write(r.join("state/build/manifest.json"), "{}").unwrap();
            std::fs::write(r.join("state/build/jobs.json"), "{}").unwrap();
            std::fs::write(r.join("state/build/pending.json"), "{}").unwrap();
            let mac = |n: &str| {
                let home = d.join(n).join("agent");
                let nas: SharedNas = Arc::new(Share::new(&r));
                let mut side =
                    Side::open(&home, &home.join("pool"), home.parent().unwrap(), "development", nas).unwrap().unwrap();
                // (Its clocks virtual: time passes as the test says, `pass`, however slowly a busy
                // Mac steps it.)
                side.set_clock(store::clock::Virtual::new());
                let mut run = Run::new(side, Role::Member, Gates::default());
                // (Its tries of the others' coordinators in its step, answered at once: reached,
                // unless a test says otherwise.)
                run.prober.inline = true;
                run.prober.probe = Arc::new(|_: &[String]| Ok(()));
                Mac { conds: HOME, run, home, root: r.clone() }
            };
            let (mut a, mut b) = (mac("a"), mac("b"));
            assert_eq!(a.go().leads, Some(1));
            b.go();
            let (ia, ib) = (a.id(), b.id());
            a.run.side.know(&ib);
            b.run.side.know(&ia);
            (a, b)
        }

        impl Mac {
            /// One loop's part: its asks taken, the step (a settle answered at the next, as the
            /// agent does), what came of it.
            pub fn go(&mut self) -> Out {
                self.run.conds = Some(self.conds);
                take(&mut self.run, &self.home);
                self.run.tend_reach();
                let out = self.run.step(self.conds.able);
                if out.settle {
                    self.run.settled = Some(
                        serde_json::json!({ "leases": { "next": 1, "leases": [] }, "costs": {}, "failed": [], "pause": null, "pause_at": 0 }),
                    );
                }
                after(&mut self.run, &out, &self.root, &self.home, None);
                out
            }
            pub fn id(&self) -> String {
                self.run.side.member().id.clone()
            }
            pub fn ask(&self, a: LeadAsk) {
                crate::control::request_lead(&self.home, a, "a test").unwrap();
            }
            pub fn asked(&self) -> Asked {
                self.run.controls.kept.asked.clone().expect("an ask")
            }
            pub fn view(&self) -> View {
                self.run.controls.view.clone().expect("a view")
            }
            /// Both its clocks moved on `s` seconds, its run stepped every `TICK_S` meanwhile, as its
            /// pool's own thread steps it whatever its agent's loop does (a lead not stepped for
            /// five minutes awake re-asserts: crate::pool::driver's `STALL_S`). The last step is
            /// the test's.
            pub fn pass(&mut self, mut s: u64) {
                while s > 0 {
                    let k = s.min(crate::agent::pool::TICK_S);
                    self.run.side.ahead += k;
                    s -= k;
                    if s > 0 {
                        self.go();
                    }
                }
            }
        }

        /// `Mac::pass` on the Macs together: their clocks moved on `s` seconds a `TICK_S` at a time,
        /// each stepped in turn meanwhile (the last steps the test's).
        pub(super) fn pass_all(macs: &mut [&mut Mac], mut s: u64) {
            while s > 0 {
                let k = s.min(crate::agent::pool::TICK_S);
                s -= k;
                for m in macs.iter_mut() {
                    m.run.side.ahead += k;
                    if s > 0 {
                        m.go();
                    }
                }
            }
        }

        /// Steps the Macs in turn until `done` (their listings made off the loop meanwhile, as long
        /// as a busy Mac takes: five minutes is a watchdog, not a measure).
        pub(super) fn until(macs: &mut [&mut Mac], done: impl Fn(&[&mut Mac]) -> bool) {
            let end = std::time::Instant::now() + std::time::Duration::from_secs(300);
            loop {
                for m in macs.iter_mut() {
                    m.go();
                }
                if done(macs) {
                    return;
                }
                assert!(
                    std::time::Instant::now() < end,
                    "not done in five minutes: {:?}",
                    macs.iter().map(|m| m.run.controls.kept.asked.clone()).collect::<Vec<_>>()
                );
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
        }

        #[test]
        fn the_lead_handed_over_each_way_from_either_mac() {
            let d = tempfile::tempdir().unwrap();
            let (mut a, mut b) = pool(d.path());
            let (ia, ib) = (a.id(), b.id());
            // The views: A leads, B can take it.
            let v = b.view();
            assert_eq!((v.term, v.leading, v.lead.as_ref().map(|l| l.member.clone())), (1, false, Some(ia.clone())));
            let mb = v.members.iter().find(|m| m.me).unwrap();
            assert!(mb.can_lead && mb.state == "home on power", "{mb:?}");
            assert!(v.members.iter().any(|m| m.leads && !m.can_lead && m.why_not.as_deref() == Some("it leads")));
            assert_eq!(
                v.takeover.as_ref().and_then(|t| t.force.clone()).map(|f| f.contains("in touch")),
                Some(true),
                "the lead in touch: a takeover needs force"
            );
            // "Make This Mac Lead" on B: passed on to A by mail; A hands over; B leads term 2.
            b.ask(LeadAsk::Give { to: ib.clone() });
            b.go();
            assert_eq!(b.asked().state, State::Passed, "{:?}", b.asked());
            until(&mut [&mut a, &mut b], |m| m[1].run.side.driver().leads() == Some(2));
            until(&mut [&mut a, &mut b], |m| m[1].asked().state == State::Done);
            assert!(b.asked().said.contains("leads term 2"), "{:?}", b.asked());
            assert_eq!(a.run.side.driver().leads(), None);
            let term2: crate::pool::term::Term =
                serde_json::from_slice(&std::fs::read(a.root.join("state/build/terms/2.json")).unwrap()).unwrap();
            assert!(term2.how.starts_with("handed over by") && term2.member == ib, "{term2:?}");
            assert!(
                a.run.controls.kept.change.as_ref().is_some_and(|c| c
                    .said
                    .starts_with(&format!("No longer leading term 1: handed over to {}", b.run.side.member().host))
                    && !c.said.contains(&ib)),
                "{:?}",
                a.run.controls.kept.change
            );
            // The history: A noted the handover (no coordinator here: on the NAS, kept for its next).
            let notes = std::fs::read_to_string(a.home.join("pool").join(NOTES)).unwrap();
            assert!(notes.contains("term 2: handed over by") && notes.contains("stepped down from term 1"), "{notes}");
            let day = crate::pool::journal::day(a.run.side.now()).unwrap();
            let nas = std::fs::read_to_string(a.root.join("state/coord/history").join(day).join(format!("{ia}.jsonl")))
                .unwrap();
            assert!(nas.contains("term 2: handed over by") && nas.contains("stepped down from term 1"), "{nas}");
            // And back: "Hand the Build To ▸ A" on the lead, B.
            until(&mut [&mut a, &mut b], |m| m[1].view().members.iter().any(|x| x.member == ia && x.can_lead));
            b.ask(LeadAsk::Give { to: ia.clone() });
            b.go();
            assert_eq!(b.asked().state, State::Going);
            until(&mut [&mut a, &mut b], |m| m[1].asked().state == State::Done);
            until(&mut [&mut a, &mut b], |m| m[0].run.side.driver().leads() == Some(3));
            assert!(a.view().leading && a.view().members.iter().any(|m| m.member == ib && m.can_lead));
        }

        /// A's coordinator can't be reached from B (the firewall on A: B's tries time out), while A
        /// reaches B's broker: B reports it; A, leading, is reported and hands the lead to B, which
        /// A reaches; B leads from then on, A never handed the lead back while B can't reach it.
        #[test]
        fn a_lead_members_cannot_reach_is_reported_and_hands_the_lead_to_one_they_reach() {
            use crate::pool::reachable::{Cause, AWAY_S, REPORT_S};
            let d = tempfile::tempdir().unwrap();
            let (mut a, mut b) = pool(d.path());
            let (ia, ib) = (a.id(), b.id());
            let broker = |u: &str| crate::coord::brokers::Broker { urls: vec![u.to_string()], ..Default::default() };
            a.run.side.broker = Some(broker("http://mac-a:8090"));
            b.run.side.broker = Some(broker("http://mac-b:8090"));
            b.run.cx.lead_urls = vec!["http://mac-a:8090".into()];
            b.run.prober.probe = Arc::new(|u: &[String]| {
                if u.iter().any(|u| u.contains("mac-a")) {
                    Err((Cause::TimedOut, "timeout: connect".into()))
                } else {
                    Ok(())
                }
            });
            // Less than REPORT_S of failures: nothing said yet.
            pass_all(&mut [&mut a, &mut b], 60);
            assert!(a.view().unreached.is_empty() && b.view().unreached.is_empty());
            // Past it: both views say who can't reach whom, and why; A's member can't be handed
            // the lead (it leads now, so it says that).
            pass_all(&mut [&mut a, &mut b], REPORT_S + 60);
            pass_all(&mut [&mut a, &mut b], 40);
            (a.go(), b.go());
            for v in [a.view(), b.view()] {
                assert!(
                    v.unreached.len() == 1
                        && v.unreached[0].contains("coordinator is unreachable from")
                        && v.unreached[0].contains("connection timed out (")
                        && v.unreached[0].contains("asleep, off the network, or its firewall blocking Scenic")
                        && v.unreached[0].contains("gets no work while"),
                    "{:?}",
                    v.unreached
                );
                let ma = v.members.iter().find(|m| m.member == ia).unwrap();
                assert!(ma.unreached.as_deref().is_some_and(|w| w.starts_with("unreachable from")), "{ma:?}");
                assert!(v.members.iter().find(|m| m.member == ib).is_some_and(|m| m.unreached.is_none() && m.can_lead));
            }
            assert!(said(&b.view(), b.run.side.now())
                .iter()
                .any(|l| l.contains("System Settings › Network › Firewall")));
            assert_eq!(a.run.side.driver().leads(), Some(1), "not yet: reported for less than AWAY_S");
            // Reported AWAY_S: A hands the lead to B by itself.
            pass_all(&mut [&mut a, &mut b], AWAY_S);
            until(&mut [&mut a, &mut b], |m| m[1].run.side.driver().leads() == Some(2));
            let term2: crate::pool::term::Term =
                serde_json::from_slice(&std::fs::read(a.root.join("state/build/terms/2.json")).unwrap()).unwrap();
            assert!(term2.member == ib && term2.how.starts_with("handed over by"), "{term2:?}");
            assert!(a.run.controls.kept.away_at > 0);
            // B leads; A, a member whose broker B can't reach, can't be handed the lead: not by the
            // views, the asks, nor the driver.
            until(&mut [&mut a, &mut b], |m| {
                m[1].view().members.iter().any(|x| {
                    x.member == ia
                        && !x.can_lead
                        && x.why_not.as_deref().is_some_and(|w| w.starts_with("unreachable from"))
                })
            });
            let why = b.run.side.hand_to(&ia).unwrap_err();
            assert!(why.contains("connection timed out"), "{why}");
            b.ask(LeadAsk::Give { to: ia.clone() });
            b.go();
            assert!(
                b.asked().state == State::Refused && b.asked().said.contains("unreachable from"),
                "{:?}",
                b.asked()
            );
            a.ask(LeadAsk::Give { to: ia.clone() });
            a.go();
            assert_eq!(a.asked().state, State::Refused, "{:?}", a.asked());
            // Hours on, B still leads: no flapping back.
            pass_all(&mut [&mut a, &mut b], 4 * 3600);
            assert_eq!((b.run.side.driver().leads(), a.run.side.driver().leads()), (Some(2), None));
            assert!(!a.root.join("state/build/terms/3.json").exists());
            // A reachable again (its firewall allows Scenic): the report clears at the first try
            // that answers, and A can lead again.
            b.run.prober.probe = Arc::new(|_: &[String]| Ok(()));
            pass_all(&mut [&mut a, &mut b], 60);
            until(&mut [&mut a, &mut b], |m| m[1].view().members.iter().any(|x| x.member == ia && x.can_lead));
            assert!(b.view().unreached.is_empty());
        }

        /// A lead others reach, though one member can't reach a third's broker: nothing moves; and a
        /// lead reported with no member others reach to take it keeps it.
        #[test]
        fn a_lead_keeps_the_lead_with_no_member_others_reach() {
            use crate::pool::reachable::{Cause, AWAY_S, REPORT_S};
            let d = tempfile::tempdir().unwrap();
            let (mut a, mut b) = pool(d.path());
            let ia = a.id();
            // B has no broker: A tries nothing of it, so nobody reached it lately.
            a.run.side.broker =
                Some(crate::coord::brokers::Broker { urls: vec!["http://mac-a:8090".into()], ..Default::default() });
            b.run.cx.lead_urls = vec!["http://mac-a:8090".into()];
            b.run.prober.probe = Arc::new(|_: &[String]| Err((Cause::Refused, "connection refused".into())));
            pass_all(&mut [&mut a, &mut b], REPORT_S + AWAY_S + 3600);
            let v = a.view();
            assert!(
                v.unreached.len() == 1
                    && v.unreached[0].contains("connection refused (")
                    && v.unreached[0].contains("firewall refusing Scenic"),
                "{:?}",
                v.unreached
            );
            assert_eq!(a.run.side.driver().leads(), Some(1));
            assert_eq!(a.run.controls.kept.away_at, 0);
            assert!(v.members.iter().any(|m| m.member == ia && m.leads));
        }

        #[test]
        fn a_refused_ask_says_why_and_reaches_no_driver() {
            let d = tempfile::tempdir().unwrap();
            let (mut a, mut b) = pool(d.path());
            let ib = b.id();
            a.ask(LeadAsk::Give { to: "Nobodys-Mac".into() });
            a.go();
            assert_eq!(a.asked().state, State::Refused);
            assert!(a.asked().said.contains("no member of the pool is called Nobodys-Mac"));
            // (Both of this test's members have this Mac's host name: named by it, neither.)
            a.ask(LeadAsk::Give { to: a.run.side.member().host.to_lowercase() });
            a.go();
            assert!(
                a.asked().state == State::Refused && a.asked().said.contains("2 members are called"),
                "{:?}",
                a.asked()
            );
            // To the lead itself.
            a.ask(LeadAsk::Give { to: a.id() });
            let o = a.go();
            assert!(a.asked().state == State::Refused && a.asked().said.contains("it leads term 1"), "{:?}", a.asked());
            assert!(!o.events.iter().any(|e| matches!(e, Event::Handover { .. })));
            // B out of touch (eleven minutes on A's clock since its beat).
            a.pass(660);
            a.ask(LeadAsk::Give { to: ib.clone() });
            let o = a.go();
            assert!(a.asked().state == State::Refused && a.asked().said.contains("out of touch"), "{:?}", a.asked());
            assert!(a.view().members.iter().any(|m| m.member == ib && !m.can_lead && m.state == "out of touch"));
            assert!(
                !o.events.iter().any(|e| matches!(e, Event::Handover { .. })) && a.run.side.driver().leads() == Some(1)
            );
            // B's ask to take over, unforced, the lead in touch: refused, saying force is needed.
            b.ask(LeadAsk::Take { force: false, downgrade: false });
            b.go();
            assert!(
                b.asked().state == State::Refused && b.asked().said.contains("needs the owner's force"),
                "{:?}",
                b.asked()
            );
            assert_eq!(b.run.side.driver().leads(), None);
        }

        /// A member that can't lead now by its heartbeat's conditions (away from home: its driver
        /// wouldn't answer an offer) is shown so, and an ask to hand it the lead is refused up
        /// front, saying why, not offered and given up a minute on.
        #[test]
        fn a_member_that_cant_lead_now_is_shown_so_and_refused_up_front() {
            let d = tempfile::tempdir().unwrap();
            let (mut a, mut b) = pool(d.path());
            let ib = b.id();
            b.conds = Conds { home: false, ac: true, battery: None, able: false, held: false, target_mb: 0 };
            b.go();
            a.pass(31);
            a.go();
            let mb = a.view().members.into_iter().find(|m| m.member == ib).unwrap();
            assert!(
                !mb.can_lead
                    && mb.state == "away"
                    && mb.why_not.as_deref().is_some_and(|w| w.contains("away from home")),
                "{mb:?}"
            );
            a.ask(LeadAsk::Give { to: ib.clone() });
            let o = a.go();
            assert!(a.asked().state == State::Refused && a.asked().said.contains("away from home"), "{:?}", a.asked());
            assert!(
                !o.events.iter().any(|e| matches!(e, Event::Handover { .. })) && a.run.side.driver().leads() == Some(1)
            );
            // On battery below the least a lead runs on, the same.
            b.conds = Conds { home: true, ac: false, battery: Some(12), able: false, held: false, target_mb: 0 };
            b.go();
            a.ask(LeadAsk::Give { to: ib.clone() });
            a.go();
            assert!(
                a.asked().state == State::Refused && a.asked().said.contains("on battery at 12%"),
                "{:?}",
                a.asked()
            );
            // Home on power again: handed over.
            b.conds = HOME;
            b.go();
            a.ask(LeadAsk::Give { to: ib.clone() });
            a.go();
            assert_eq!(a.asked().state, State::Going, "{:?}", a.asked());
        }

        #[test]
        fn a_handover_whose_target_doesnt_answer_is_given_up() {
            let d = tempfile::tempdir().unwrap();
            let (mut a, b) = pool(d.path());
            a.ask(LeadAsk::Give { to: b.id() });
            let o = a.go();
            assert!(o.events.iter().any(|e| matches!(e, Event::Handover { what: "offered", .. })), "{:?}", o.events);
            assert_eq!(a.asked().state, State::Going);
            assert!(
                a.view().handing.as_ref().is_some_and(|h| h.stage == Stage::Offered && h.to == b.id()),
                "{:?}",
                a.view().handing
            );
            // B never answers: the offer's time on, it's given up; A leads term 1 still.
            a.pass(crate::pool::handover::OFFER_S + 1);
            a.go();
            assert_eq!(a.asked().state, State::Failed, "{:?}", a.asked());
            assert!(a.asked().said.contains("given up"));
            assert_eq!(a.run.side.driver().leads(), Some(1));
            assert!(a.view().handing.is_none());
        }

        #[test]
        fn a_handover_not_taken_up_is_taken_back() {
            let d = tempfile::tempdir().unwrap();
            let (mut a, mut b) = pool(d.path());
            a.ask(LeadAsk::Give { to: b.id() });
            a.go();
            // B answers once, then is gone (asleep).
            b.go();
            until(&mut [&mut a], |m| m[0].run.side.driver().leads().is_none());
            assert!(a.asked().said.contains("passed"), "{:?}", a.asked());
            a.pass(130);
            until(&mut [&mut a], |m| m[0].run.side.driver().leads() == Some(3));
            assert_eq!(a.asked().state, State::Failed);
            assert!(a.asked().said.contains("took the build back"), "{:?}", a.asked());
        }

        #[test]
        fn a_member_takes_over_a_lead_out_of_touch() {
            let d = tempfile::tempdir().unwrap();
            let (a, mut b) = pool(d.path());
            // A is silent; eleven minutes on B's clock, B not stepped meanwhile (asleep itself), so
            // it hasn't watched A's beat long enough to take over by itself (`SILENT_SEEN_S`).
            b.run.side.ahead += 660;
            b.go();
            let v = b.view();
            assert!(v.no_lead.as_ref().is_some_and(|w| w.contains("out of touch")), "{v:?}");
            assert_eq!(v.takeover, Some(Takeover::default()), "no force needed");
            b.ask(LeadAsk::Take { force: false, downgrade: false });
            b.go();
            until(&mut [&mut b], |m| m[0].asked().state == State::Done);
            assert_eq!(b.run.side.driver().leads(), Some(2));
            let term2: crate::pool::term::Term =
                serde_json::from_slice(&std::fs::read(b.root.join("state/build/terms/2.json")).unwrap()).unwrap();
            assert!(term2.how.starts_with("taken over by"), "{term2:?}");
            assert!(b.run.controls.kept.change.as_ref().is_some_and(|c| c.said.contains("This Mac leads term 2")));
            drop(a);
        }

        #[test]
        fn a_lead_whose_coordinator_answers_is_late_not_gone() {
            // (#191: the lead's heartbeat went stale during its long plan steps while it granted
            // and renewed as ever; the other Mac said "No lead … out of touch".)
            let d = tempfile::tempdir().unwrap();
            let (a, mut b) = pool(d.path());
            // (B paused: it can't lead, so doesn't take a silent lead over by itself.)
            b.conds = Conds { held: true, able: false, ..HOME };
            b.pass(660);
            let now = b.run.side.now();
            b.run.cx.lead_answered = Some(now - 20);
            b.run.controls.stale = true;
            b.go();
            let v = b.view();
            assert_eq!(v.no_lead, None, "{v:?}");
            assert!(
                v.quiet.as_ref().is_some_and(
                    |q| q.contains("heartbeat is late") && q.contains("its coordinator answered this Mac 20 s ago")
                ),
                "{v:?}"
            );
            assert!(said(&v, now).iter().any(|l| Some(l) == v.quiet.as_ref()));
            // Taking over unforced: refused, saying so; forced, the owner's call.
            b.ask(LeadAsk::Take { force: false, downgrade: false });
            b.go();
            assert!(
                b.asked().state == State::Refused && b.asked().said.contains("coordinator answered this Mac"),
                "{:?}",
                b.asked()
            );
            assert_eq!(b.run.side.driver().leads(), None);
            // Late for an hour: the alert all the same, its takeover forced.
            b.pass(ALERT_S);
            b.run.cx.lead_answered = Some(b.run.side.now() - 20);
            b.run.controls.stale = true;
            b.go();
            let w = b.view().alert.expect("an alert");
            assert!(w.contains("coordinator still answers") && w.contains("scenic lead take --force"), "{w}");
            let now = b.run.side.now();
            // Its coordinator not heard from for as long as its heartbeat: no lead, as before.
            b.run.cx.lead_answered = Some(now - ANSWERED_S);
            b.run.controls.stale = true;
            b.go();
            assert!(
                b.view().no_lead.as_ref().is_some_and(|w| w.contains("out of touch")) && b.view().quiet.is_none(),
                "{:?}",
                b.view()
            );
            drop(a);
        }

        #[test]
        fn no_lead_for_an_hour_is_an_alert_saying_what_to_do() {
            let d = tempfile::tempdir().unwrap();
            let (a, mut b) = pool(d.path());
            // A silent, B paused (it can't lead, so takes nothing over by itself): out of touch at
            // eleven minutes, no alert yet; an hour after its last beat, the alert, with the command.
            b.conds = Conds { held: true, able: false, ..HOME };
            b.pass(660);
            b.go();
            assert!(b.view().no_lead.is_some() && b.view().alert.is_none(), "{:?}", b.view());
            b.pass(ALERT_S - 660 + VIEW_S);
            b.go();
            let w = b.view().alert.expect("an alert");
            assert!(
                w.starts_with("No lead for 60 min: ")
                    && w.contains("out of touch")
                    && w.contains("; no Mac took it over by itself")
                    && w.contains("`scenic lead take` on this Mac"),
                "{w}"
            );
            assert!(said(&b.view(), b.run.side.now())[0] == w, "`scenic lead` and `scenic status` say it first");
            drop(a);
        }

        #[test]
        fn a_member_that_can_lead_takes_a_silent_lead_over_by_itself() {
            // (19 Oct: the M4 at its login screen after a restart, the M1 leading nothing for 85
            // min.) A silent, B able: B leads within the bound, no alert.
            let d = tempfile::tempdir().unwrap();
            let (a, mut b) = pool(d.path());
            let bound = crate::pool::driver::SILENT_S
                + crate::pool::driver::SILENT_SEEN_S
                + 2 * crate::pool::driver::AUTO_RANK_S
                + 2 * crate::agent::pool::TICK_S;
            b.pass(bound);
            b.go();
            assert_eq!(b.run.side.driver().leads(), Some(2));
            let term2: crate::pool::term::Term =
                serde_json::from_slice(&std::fs::read(b.root.join("state/build/terms/2.json")).unwrap()).unwrap();
            assert!(term2.how.contains("silent for"), "{term2:?}");
            assert_eq!(b.view().alert, None);
            drop(a);
        }

        #[test]
        fn a_lead_on_battery_hands_over_by_itself_only_below_the_threshold() {
            let d = tempfile::tempdir().unwrap();
            let (mut a, mut b) = pool(d.path());
            // On battery above the threshold, or away from home with the NAS: A keeps the lead.
            for c in [Conds { ac: false, battery: Some(40), ..HOME }, Conds { home: false, able: false, ..HOME }] {
                a.conds = c;
                pass_all(&mut [&mut a, &mut b], 3600);
                for _ in 0..4 {
                    a.go();
                    b.go();
                }
                assert_eq!(a.run.side.driver().leads(), Some(1), "{c:?}");
            }
            // Below it: handed to B by itself.
            a.conds = Conds { ac: false, battery: Some(crate::agent::cond::BATTERY_MIN - 1), able: false, ..HOME };
            pass_all(&mut [&mut a, &mut b], VIEW_S);
            until(&mut [&mut a, &mut b], |m| m[1].run.side.driver().leads() == Some(2));
            assert!(a.asked().by.starts_with("the pool by itself (its battery at 29 %"), "{:?}", a.asked());
            // B, its new lead, on battery above the threshold: no handing back.
            b.conds = Conds { ac: false, battery: Some(40), ..HOME };
            a.conds = HOME;
            pass_all(&mut [&mut a, &mut b], 3600);
            for _ in 0..4 {
                a.go();
                b.go();
            }
            assert_eq!(b.run.side.driver().leads(), Some(2));
        }

        #[test]
        fn an_ask_and_its_state_outlive_the_process() {
            let d = tempfile::tempdir().unwrap();
            let (_a, mut b) = pool(d.path());
            b.ask(LeadAsk::Give { to: b.id() });
            b.go();
            let kept = Controls::open(&b.home.join("pool")).kept;
            assert_eq!(kept.asked.map(|a| a.state), Some(State::Passed));
        }

        #[test]
        fn the_regions_switch_bars_older_apps_from_the_lead() {
            let d = tempfile::tempdir().unwrap();
            let (mut a, mut b) = pool(d.path());
            pass_all(&mut [&mut a, &mut b], 60);
            let ib = b.id();
            let root = a.root.clone();
            let switch_to = |file: &str| {
                let p = root.join("state/build/manifest.json");
                let mut m: std::collections::BTreeMap<String, String> =
                    std::fs::read(&p).ok().and_then(|x| serde_json::from_slice(&x).ok()).unwrap_or_default();
                m.insert(crate::inputs::switched_logical(crate::inputs::regions::UNIT), file.to_string());
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, serde_json::to_vec(&m).unwrap()).unwrap();
            };
            // Proved on an app newer than theirs: B can't be handed the lead, and the view says why.
            std::fs::write(a.root.join("sw.json"), br#"{"same": true, "app": "29991231-0000-aaaaaaa"}"#).unwrap();
            switch_to("sw.json");
            assert!(a.run.side.hand_to(&ib).unwrap_err().contains(
                "older than the one the newer readers' switch (the regions', the elevation datasets') was recorded on"
            ));
            a.run.controls.stale = true;
            a.go();
            let v = a.view();
            let mb = v.members.iter().find(|m| m.member == ib).unwrap();
            assert!(!mb.can_lead && mb.state == "app too old", "{mb:?}");
            // The switch's file unreadable: unknown, refused, and said.
            switch_to("gone.json");
            assert!(a.run.side.hand_to(&ib).unwrap_err().contains("can't be read now"));
            a.run.controls.stale = true;
            a.go();
            let v = a.view();
            assert!(
                v.members.iter().all(|m| m.state.starts_with("the readers' switches' app unknown")),
                "{:?}",
                v.members
            );
        }
    }
}
