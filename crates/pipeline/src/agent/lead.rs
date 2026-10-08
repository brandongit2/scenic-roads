//! The owner's controls of the pool's lead (docs/pool.md §6.3–§6.5, §10, §11): their asks reaching
//! the driver, where each stands, and what every view shows of the pool.
//!
//! - **The asks** (crate::control::LeadAsk): from this Mac's menu, `scenic lead` and the map, a file
//!   in the agent's folder (`control::LEAD_REQUEST`); from the build page, its lead's coordinator
//!   (`/work/lead`). The agent takes them at its next loop (`Controls::take`) and checks each as
//!   the driver's step would (`Driver::hand_to`, `Driver::takeover`): one refused never reaches it,
//!   and says why; one that passes goes to the next step as the owner's ask, the driver handing
//!   over (on the lead) or passing it on to the lead by mail (on a member, §9: the pool's HTTP API
//!   is planned). Where it stands (`Asked`) is followed from the steps' events and the terms.
//! - **The view** (`View`, in the agent's status): the lead and its term; each member, read by id
//!   from its heartbeat (never listed), with its state and whether the lead can be handed to it,
//!   and why not; what a takeover from this Mac needs; a handover under way; "no lead", and why; the
//!   proactive offer (§6.5); the ask's state; the last change of lead in words. Made again every
//!   `VIEW_S`, and at once after an ask or a change.
//! - **The proactive offer, automatic** when the owner turns it on (`AUTO` on the NAS, `scenic lead
//!   auto on`): the lead asks the handover itself once the offer has stood `AUTO_AFTER_S`, never
//!   while a handover or an ask is under way, nor within `CALM_S` of the last change of lead (`auto`).
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
use crate::pool::term;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The owner's switch for the proactive offer to be taken by itself, on the NAS (off while missing).
pub const AUTO: &str = "state/pool/auto-handover";
/// How long the offer stands before the lead takes it by itself, the switch on (s).
pub const AUTO_AFTER_S: u64 = 300;
/// No automatic handover within this of the last change of lead, or of the last automatic ask (s):
/// so the lead can't flap between two Macs on the edge of their conditions.
pub const CALM_S: u64 = 1800;
/// The view is made again this often (s): a heartbeat read per member.
const VIEW_S: u64 = 30;
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
    /// The offer as first seen standing (its target, since when), and the last automatic ask.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offered: Option<(String, u64)>,
    pub auto_at: u64,
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

/// The proactive offer (§6.5): the lead is away or on battery, and another member is home on power.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    pub to: String,
    pub host: String,
    pub why: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handing: Option<Handing>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offer: Option<Offer>,
    /// The offer is taken by itself (`AUTO`).
    pub auto: bool,
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
        let kept: Kept = std::fs::read(dir.join(KEPT)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Controls { kept_written: Some(kept.clone()), kept, ..Default::default() }
    }

    fn save(&mut self, dir: &Path) {
        if self.kept_written.as_ref() == Some(&self.kept) {
            return;
        }
        let r = serde_json::to_vec(&self.kept).map_err(anyhow::Error::from).and_then(|b| crate::whole::write(&dir.join(KEPT), &b));
        match r {
            Ok(()) => self.kept_written = Some(self.kept.clone()),
            Err(e) => eprintln!("pool: keeping the lead asks' state: {e:#}"),
        }
    }
}

/// A member named `name` (its id, or its host name, any case): its id.
fn resolve(run: &Run, name: &str) -> Option<String> {
    if crate::pool::is_member_id(name) {
        return Some(name.to_string());
    }
    let m = run.controls.view.as_ref().and_then(|v| v.members.iter().find(|m| m.host.eq_ignore_ascii_case(name)).map(|m| m.member.clone()));
    m.or_else(|| run.side.members().iter().find(|id| run.side.heartbeat(id).is_some_and(|h| h.pool.host.eq_ignore_ascii_case(name))).cloned())
}

fn host_of(run: &Run, id: &str) -> String {
    run.controls.view.as_ref().and_then(|v| v.members.iter().find(|m| m.member == id).map(|m| m.host.clone())).or_else(|| run.side.heartbeat(id).map(|h| h.pool.host)).filter(|h| !h.is_empty()).unwrap_or_else(|| id.to_string())
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
                None => (State::Refused, format!("no member of the pool is called {to}"), None, None),
                Some(id) => {
                    let host = host_of(run, &id);
                    match run.side.hand_to(&id) {
                        Err(why) => (State::Refused, format!("the lead can't be handed to {host}: {why}"), Some(id), None),
                        Ok(()) if leads => (State::Going, format!("handing the lead to {host}"), Some(id.clone()), Some(Ask::HandTo(id))),
                        Ok(()) => (State::Passed, format!("asked {} to hand the lead to {}", lead_host.unwrap_or_else(|| "the lead".into()), if id == me { "this Mac".to_string() } else { host }), Some(id.clone()), Some(Ask::HandTo(id))),
                    }
                }
            },
            LeadAsk::Take { force, downgrade } => {
                let t = run.side.takeover();
                match (t.refused, t.force, t.downgrade) {
                    (Some(why), _, _) => (State::Refused, format!("this Mac can't take the lead over: {why}"), None, None),
                    (_, Some(why), _) if !force => (State::Refused, format!("taking the lead over needs the owner's force (scenic lead take --force): {why}"), None, None),
                    (_, _, Some(why)) if !downgrade => (State::Refused, format!("taking the lead over needs the owner's downgrade (scenic lead take --downgrade): {why}"), None, None),
                    _ => (State::Going, "taking the lead over".to_string(), Some(me), Some(Ask::TakeOver { force: *force, downgrade: *downgrade })),
                }
            }
        };
        eprintln!("pool: {} asked: {said}", r.by);
        run.asks.extend(ask);
        run.controls.kept.asked = Some(Asked { ask: r.ask, by: r.by, at: r.at.min(now).max(1), since: now, state, said, to });
        run.controls.stale = true;
    }
}

/// After a step: the ask's state followed, the last change of lead noted, the history's terms
/// written, the view made again when due, and (the lead, the owner's switch on) the offer taken by
/// itself. `coord`: this process's coordinator (the lead's), whose history the terms go to.
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
            let why = run.side.members().iter().fold(why.clone(), |w, id| if w.contains(id.as_str()) { w.replace(id.as_str(), &host_of(run, id)) } else { w });
            let next = cur.as_ref().filter(|t| t.term > *term).map(|t| format!("; term {} is {}'s ({})", t.term, t.host, t.how)).unwrap_or_default();
            run.controls.kept.change = Some(Change { at: now, said: format!("No longer leading term {term}: {why}{next}") });
        }
        if let Event::Made { term, how } = e {
            if cur.as_ref().is_some_and(|t| t.term == *term && t.member == me) {
                run.controls.kept.change = Some(Change { at: now, said: format!("This Mac leads term {term} ({how})") });
            }
        }
    }
    // Where the ask stands.
    // (A term naming the member asked for: whether it leads it, as this Mac or its heartbeat says.)
    let to = run.controls.kept.asked.as_ref().and_then(|a| a.to.clone());
    let to_leads = match (&to, &cur) {
        (Some(to), Some(t)) if t.member == *to => if *to == me { out.leads == Some(t.term) } else { run.side.heartbeat(to).is_some_and(|h| h.pool.leads == Some(t.term)) },
        _ => false,
    };
    if let Some(a) = run.controls.kept.asked.as_mut() {
        let was = a.state;
        follow(a, out, cur.as_ref(), to_leads, &me, now);
        if a.state != was {
            a.since = now;
            eprintln!("pool: the ask {}: {}", serde_json::to_value(a.state).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(), a.said);
        }
        if matches!(a.state, State::Refused | State::Done | State::Failed) && now.saturating_sub(a.since) > SHOWN_S {
            run.controls.kept.asked = None;
        }
    }
    if out.events.iter().any(|e| matches!(e, Event::Made { .. } | Event::SteppedDown { .. } | Event::Handover { .. } | Event::TookUp { .. })) {
        run.controls.stale = true;
    }
    // The view.
    let due = run.controls.stale || run.controls.view.as_ref().is_none_or(|v| now.abs_diff(v.at) >= VIEW_S);
    if due {
        let auto = run.side.nas().exists(AUTO).unwrap_or(false);
        let v = view(run, auto);
        run.controls.view = Some(v);
        run.controls.stale = false;
    }
    // The offer, taken by itself (the lead's, the switch on).
    if let Some(v) = run.controls.view.clone() {
        let going = run.controls.kept.asked.as_ref().is_some_and(|a| matches!(a.state, State::Going | State::Passed));
        let last_change = v.lead.as_ref().map_or(0, |l| l.since);
        if v.leading {
            if let Some(to) = auto(v.offer.as_ref(), v.auto, &mut run.controls.kept, now, last_change, going || v.handing.is_some()) {
                let host = host_of(run, &to);
                let by = format!("the pool by itself ({})", v.offer.as_ref().map_or(String::new(), |o| o.why.clone()));
                run.controls.page.push(LeadRequest { ask: LeadAsk::Give { to: to.clone() }, by: by.clone(), at: now });
                eprintln!("pool: the offer stood {} min: handing the lead to {host} by itself", AUTO_AFTER_S / 60);
            }
        } else {
            run.controls.kept.offered = None;
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
                (a.state, a.said) = (State::Done, format!("{} leads term {}", cur.map(|t| t.host.clone()).unwrap_or_default(), cur.map_or(0, |t| t.term)));
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
        Some(c) if !c.home => "away".into(),
        Some(c) if !c.ac => c.battery.map_or("on battery".to_string(), |p| format!("on battery ({p} %)")),
        Some(_) => "home on power".into(),
    }
}

/// The view, made now.
fn view(run: &Run, auto: bool) -> View {
    let side = &run.side;
    let now = side.now();
    let me = side.member().id.clone();
    let cur = side.driver().current().clone();
    let leading = side.driver().leads().is_some();
    let lead = cur.lead.as_ref().map(|t| LeadOf { term: t.term, member: t.member.clone(), host: t.host.clone(), app: t.app.clone(), since: t.since, how: t.how.clone() });
    let mut ids: Vec<String> = side.members().iter().cloned().collect();
    if let Some(t) = &cur.lead {
        if !ids.contains(&t.member) {
            ids.push(t.member.clone());
        }
    }
    let mut beats: Vec<(String, Heartbeat)> = Vec::new();
    let mut members: Vec<Member> = Vec::new();
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
            match side.hand_to(&id) {
                Ok(()) => (true, None),
                Err(w) => (false, Some(w)),
            }
        };
        let host = if h.pool.host.is_empty() { if id == me { side.member().host.clone() } else { id.clone() } } else { h.pool.host.clone() };
        members.push(Member {
            member: id.clone(),
            host,
            app: h.pool.app.clone(),
            beat: h.pool.beat,
            me: id == me,
            leads: is_lead,
            state: state_of(&h, now, cur.lead.as_ref().map(|t| t.app.as_str())),
            out_of_touch: h.pool.beat == 0 || h.pool.out_of_touch(now),
            away: h.conds.is_some_and(|c| !c.home),
            can_lead: can,
            why_not: why,
            conds: h.conds,
        });
        beats.push((id, h));
    }
    members.sort_by(|a, b| b.me.cmp(&a.me).then(b.leads.cmp(&a.leads)).then(a.host.cmp(&b.host)));
    let lead_beat = cur.lead.as_ref().and_then(|t| beats.iter().find(|(id, _)| *id == t.member)).map(|(_, h)| h.clone());
    let no_lead = match (&cur.lead, &lead_beat) {
        _ if cur.term == 0 => None,
        (None, _) => Some(format!("term {} can't be read whole", cur.term)),
        (Some(t), _) if leading || t.member == me => None,
        (Some(t), None) => Some(format!("no heartbeat from {}", t.host)),
        (Some(t), Some(h)) if h.pool.beat == 0 => Some(format!("no heartbeat from {}", t.host)),
        (Some(t), Some(h)) if h.pool.out_of_touch(now) => Some(format!("{} is out of touch: last heard from {} s ago", t.host, now.saturating_sub(h.pool.beat))),
        (Some(t), Some(h)) if h.pool.stood_down == Some(t.term) => Some(format!("{} stood down from term {} (its app is older than the term's)", t.host, t.term)),
        _ => None,
    };
    let handing = lead_beat.as_ref().and_then(|h| h.pool.handing_to.as_ref()).map(|ht| Handing { to: ht.to.clone(), host: members.iter().find(|m| m.member == ht.to).map_or(ht.to.clone(), |m| m.host.clone()), term: ht.term, stage: ht.stage, since: ht.since });
    let takeover = (!leading).then(|| {
        let t = side.takeover();
        Takeover { refused: t.refused, force: t.force, downgrade: t.downgrade }
    });
    let offer = offer(lead_beat.as_ref(), &members);
    View { at: now, term: cur.term, lead, leading, members, takeover, no_lead, handing, offer, auto, asked: run.controls.kept.asked.clone(), change: run.controls.kept.change.clone() }
}

/// The proactive offer (§6.5): the lead's heartbeat says it's away from home or on battery, no
/// handover is under way, and another member is home on power, able to lead, and the lead can be
/// handed to it: the first such, by host.
pub fn offer(lead: Option<&Heartbeat>, members: &[Member]) -> Option<Offer> {
    let l = lead?;
    let c = l.conds?;
    if (c.home && c.ac) || l.pool.handing_to.is_some() || l.pool.leads.is_none() {
        return None;
    }
    let why = if !c.home { format!("{} is away from home", l.pool.host) } else { format!("{} is on battery", l.pool.host) };
    members.iter().filter(|m| !m.leads && m.can_lead && m.conds.is_some_and(|c| c.home && c.ac && c.able)).min_by(|a, b| a.host.cmp(&b.host)).map(|m| Offer { to: m.member.clone(), host: m.host.clone(), why })
}

/// The offer taken by itself (the owner's switch, `on`): its target once the same offer has stood
/// `AUTO_AFTER_S` (`kept.offered`: since when), nothing under way (`busy`: a handover, an ask), the
/// last change of lead (`last_change`: the current term's start) and the last automatic ask
/// `CALM_S` ago or more. The ask's time is kept.
pub fn auto(offer: Option<&Offer>, on: bool, kept: &mut Kept, now: u64, last_change: u64, busy: bool) -> Option<String> {
    let Some(o) = offer else {
        kept.offered = None;
        return None;
    };
    if kept.offered.as_ref().is_none_or(|(to, _)| *to != o.to) {
        kept.offered = Some((o.to.clone(), now));
    }
    let since = kept.offered.as_ref().map_or(now, |(_, t)| *t);
    let calm = now.saturating_sub(last_change) >= CALM_S && now.saturating_sub(kept.auto_at) >= CALM_S;
    if !on || busy || !calm || now.saturating_sub(since) < AUTO_AFTER_S {
        return None;
    }
    kept.auto_at = now;
    kept.offered = None;
    Some(o.to.clone())
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
            Event::Handover { term, to, what } if matches!(*what, "offered" | "over" | "given up" | "dropped") => format!("its handover of term {term} to {}: {what}", host(to)),
            _ => continue,
        };
        out.push(history::Event { t: now, kind: "term".into(), worker: Some(me.to_string()), note, ..Default::default() });
    }
    out
}

/// The terms' events noted: into this process's coordinator's history, when it has one; else
/// appended to this member's history on the NAS (`root`), and kept in its pool folder `dir` for its
/// next process's coordinator (`replay`).
fn note(events: &[history::Event], coord: Option<&crate::coord::Coordinator>, root: &Path, member: &str, dir: &Path) {
    if let Some(c) = coord {
        for e in events {
            c.note(e.clone());
        }
        return;
    }
    if let Err(e) = super::pool::append_history(root, member, events) {
        eprintln!("pool: the terms in the history on the NAS: {e:#}");
    }
    let mut b = Vec::new();
    for e in events {
        b.extend(serde_json::to_vec(e).unwrap_or_default());
        b.push(b'\n');
    }
    use std::io::Write;
    let r = std::fs::create_dir_all(dir).and_then(|_| std::fs::OpenOptions::new().create(true).append(true).open(dir.join(NOTES))).and_then(|mut f| f.write_all(&b));
    if let Err(e) = r {
        eprintln!("pool: keeping the terms' events: {e}");
    }
}

/// The terms' events an earlier process of this member kept (`note`), into this process's
/// coordinator's history (the build page's activity); their file removed.
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
pub fn own_status(home: &Path) -> Option<super::Status> {
    ["status.json", "helper.json"].iter().filter_map(|f| std::fs::read(home.join(f)).ok()).filter_map(|b| serde_json::from_slice::<super::Status>(&b).ok()).max_by_key(|s| s.beat)
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
        (_, Some(why)) => out.push(format!("No lead: {why} (`scenic lead take` takes it over)")),
        (Some(l), None) => out.push(format!("The lead: {}{} — term {}, since {} ({})", l.host, if v.leading { " (this Mac)" } else { "" }, l.term, ago(now, l.since), l.how)),
        (None, None) => out.push("No terms yet".into()),
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
        let heard = if m.out_of_touch && m.beat > 0 { format!(", last heard from {}", ago(now, m.beat)) } else { String::new() };
        let can = match (&m.why_not, m.leads) {
            (_, true) => String::new(),
            (None, _) => format!(" — can lead{}", if m.away { " (away: its duties would run slowly over Tailscale)" } else { "" }),
            (Some(w), _) => format!(" — can't lead: {w}"),
        };
        out.push(format!("  {}{tags}: {}{heard}, app {}{can}", m.host, m.state, if m.app.is_empty() { "?" } else { &m.app }));
    }
    if let Some(o) = &v.offer {
        out.push(format!("Offer: hand the build to {} ({}){}", o.host, o.why, if v.auto { "; taken by itself after five minutes (auto on)" } else { "" }));
    }
    if let Some(t) = &v.takeover {
        if v.no_lead.is_some() || t.refused.is_none() {
            let needs: Vec<String> = [t.force.as_ref().map(|w| format!("--force ({w})")), t.downgrade.as_ref().map(|w| format!("--downgrade ({w})"))].into_iter().flatten().collect();
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
        Heartbeat { pool: Beat { member: format!("m-{host}"), host: host.into(), app: "development".into(), beat, leads: None, ..Default::default() }, conds, ..Default::default() }
    }

    const HOME: Conds = Conds { home: true, ac: true, battery: Some(90), able: true };

    #[test]
    fn a_members_state_in_words() {
        let now = 10_000;
        assert_eq!(state_of(&hb("a", now - 5, Some(HOME)), now, Some("development")), "home on power");
        assert_eq!(state_of(&hb("a", now - 5, Some(Conds { ac: false, battery: Some(54), ..HOME })), now, None), "on battery (54 %)");
        assert_eq!(state_of(&hb("a", now - 5, Some(Conds { home: false, ..HOME })), now, None), "away");
        assert_eq!(state_of(&hb("a", now - 601, Some(HOME)), now, None), "out of touch");
        assert_eq!(state_of(&hb("a", now + 61, Some(HOME)), now, None), "clock wrong");
        assert_eq!(state_of(&hb("a", now, Some(HOME)), now, Some("20991231-2359-1234567")), "app too old");
        assert!(state_of(&hb("a", now, None), now, None).starts_with("conditions unknown"));
        assert_eq!(state_of(&Heartbeat::default(), now, None), "no heartbeat");
    }

    fn member(host: &str, conds: Conds, can: bool) -> Member {
        Member { member: format!("m-{host}"), host: host.into(), can_lead: can, conds: Some(conds), state: String::new(), ..Default::default() }
    }

    #[test]
    fn the_offer_when_the_lead_is_away_or_on_battery() {
        let mut lead = hb("mini", 100, Some(HOME));
        lead.pool.leads = Some(3);
        let ms = [member("air", HOME, true), member("pro", Conds { ac: false, ..HOME }, true), member("zed", HOME, false)];
        assert_eq!(offer(Some(&lead), &ms), None, "the lead home on power: none");
        lead.conds = Some(Conds { ac: false, ..HOME });
        assert_eq!(offer(Some(&lead), &ms), Some(Offer { to: "m-air".into(), host: "air".into(), why: "mini is on battery".into() }));
        lead.conds = Some(Conds { home: false, ..HOME });
        assert!(offer(Some(&lead), &ms).is_some_and(|o| o.why.contains("away")));
        // None to take it: on battery, or the lead can't be handed to it; a handover already under way.
        assert_eq!(offer(Some(&lead), &ms[1..]), None);
        lead.pool.handing_to = Some(crate::pool::beat::HandingTo { to: "m-air".into(), term: 3, offer: 1, since: 1, stage: Stage::Offered });
        assert_eq!(offer(Some(&lead), &ms), None);
        // A lead whose app says nothing of its conditions offers nothing.
        assert_eq!(offer(Some(&hb("mini", 100, None)), &ms), None);
    }

    #[test]
    fn the_offer_is_taken_by_itself_only_switched_on_after_it_stood_and_never_flapping() {
        let o = Offer { to: "m-air".into(), host: "air".into(), why: "mini is on battery".into() };
        let t0 = 1_000_000;
        let mut k = Kept::default();
        // Off: never.
        for dt in [0, 300, 3000] {
            assert_eq!(auto(Some(&o), false, &mut k, t0 + dt, 0, false), None);
        }
        // On: once it stood five minutes.
        let mut k = Kept::default();
        assert_eq!(auto(Some(&o), true, &mut k, t0, 0, false), None);
        assert_eq!(auto(Some(&o), true, &mut k, t0 + 299, 0, false), None);
        assert_eq!(auto(Some(&o), true, &mut k, t0 + 300, 0, false), Some("m-air".into()));
        // The flap guard: the lead moved (a term begun at t0 + 360); the new lead on battery at once,
        // the offer back to mini: not within half an hour of the change, nor of the last ask.
        let back = Offer { to: "m-mini".into(), host: "mini".into(), why: "air is on battery".into() };
        let change = t0 + 360;
        let mut k2 = Kept::default();
        for dt in [400, 700, 1000, 2000] {
            assert_eq!(auto(Some(&back), true, &mut k2, t0 + dt, change, false), None, "at {dt}");
        }
        assert_eq!(auto(Some(&back), true, &mut k2, change + CALM_S, change, false), Some("m-mini".into()));
        // The same process: its own last automatic ask holds it too.
        let mut k3 = Kept { auto_at: t0, ..Default::default() };
        assert_eq!(auto(Some(&o), true, &mut k3, t0 + 10, 0, false), None);
        assert_eq!(auto(Some(&o), true, &mut k3, t0 + CALM_S - 1, 0, false), None);
        assert_eq!(auto(Some(&o), true, &mut k3, t0 + CALM_S, 0, false), Some("m-air".into()));
        // Not while a handover or an ask is under way; the offer gone, its time starts again.
        let mut k4 = Kept::default();
        auto(Some(&o), true, &mut k4, t0, 0, false);
        assert_eq!(auto(Some(&o), true, &mut k4, t0 + 600, 0, true), None);
        assert_eq!(auto(None, true, &mut k4, t0 + 601, 0, false), None);
        assert_eq!(auto(Some(&o), true, &mut k4, t0 + 602, 0, false), None);
        assert_eq!(auto(Some(&o), true, &mut k4, t0 + 902, 0, false), Some("m-air".into()));
    }

    #[test]
    fn the_terms_events_in_the_history() {
        let events = [
            Event::Made { term: 4, how: "handed over by mini".into() },
            Event::Handover { term: 3, to: "m-air".into(), what: "settling" },
            Event::Handover { term: 3, to: "m-air".into(), what: "over" },
            Event::SteppedDown { term: 3, why: "handed over to m-air".into() },
            Event::Merged { applied: vec![], overtaken: 0, refused: 0, listed: false },
        ];
        let n = notes(&events, "mini", |id| id.trim_start_matches("m-").to_string(), 5);
        let said: Vec<&str> = n.iter().map(|e| e.note.as_str()).collect();
        assert_eq!(said, ["term 4: handed over by mini", "its handover of term 3 to air: over", "stepped down from term 3: handed over to air"]);
        assert!(n.iter().all(|e| e.kind == "term" && e.worker.as_deref() == Some("mini") && e.t == 5));
    }

    /// Two members of a pool in one process, A (this Mac, the writer: term 1 is its) and B, each
    /// with its folder, stepped as the agent steps them (`go`).
    mod runs {
        use super::super::*;
        use crate::agent::pool::{Gates, Role, Run, SharedNas, Side};
        use crate::pool::nas::Share;
        use std::sync::Arc;

        const HOME: Conds = Conds { home: true, ac: true, battery: None, able: true };

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
            std::fs::write(r.join("state/build/writer"), crate::agent::cond::host_name()).unwrap();
            let mac = |n: &str| {
                let home = d.join(n).join("agent");
                let nas: SharedNas = Arc::new(Share::new(&r));
                let side = Side::open(&home, &home.join("pool"), home.parent().unwrap(), "development", nas, false).unwrap().unwrap();
                Mac { conds: HOME, run: Run::new(side, Role::Member, Gates::default()), home, root: r.clone() }
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
                let out = self.run.step(true);
                if out.settle {
                    self.run.settled = Some(serde_json::json!({ "leases": { "next": 1, "leases": [] }, "costs": {}, "failed": [], "pause": null, "pause_at": 0 }));
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
            /// Both its clocks moved on `s` seconds.
            pub fn pass(&mut self, s: u64) {
                self.run.side.ahead += s;
            }
        }

        /// Steps the Macs in turn until `done`, at most 60 rounds.
        pub(super) fn until(macs: &mut [&mut Mac], done: impl Fn(&[&mut Mac]) -> bool) {
            for _ in 0..60 {
                for m in macs.iter_mut() {
                    m.go();
                }
                if done(macs) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
            panic!("not done in 60 rounds: {:?}", macs.iter().map(|m| m.run.controls.kept.asked.clone()).collect::<Vec<_>>());
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
            assert_eq!(v.takeover.as_ref().and_then(|t| t.force.clone()).map(|f| f.contains("in touch")), Some(true), "the lead in touch: a takeover needs force");
            // "Make This Mac Lead" on B: passed on to A by mail; A hands over; B leads term 2.
            b.ask(LeadAsk::Give { to: ib.clone() });
            b.go();
            assert_eq!(b.asked().state, State::Passed, "{:?}", b.asked());
            until(&mut [&mut a, &mut b], |m| m[1].run.side.driver().leads() == Some(2));
            until(&mut [&mut a, &mut b], |m| m[1].asked().state == State::Done);
            assert!(b.asked().said.contains("leads term 2"), "{:?}", b.asked());
            assert_eq!(a.run.side.driver().leads(), None);
            let term2: crate::pool::term::Term = serde_json::from_slice(&std::fs::read(a.root.join("state/build/terms/2.json")).unwrap()).unwrap();
            assert!(term2.how.starts_with("handed over by") && term2.member == ib, "{term2:?}");
            assert!(a.run.controls.kept.change.as_ref().is_some_and(|c| c.said.starts_with("No longer leading term 1: handed over to MacBook") && !c.said.contains(&ib)), "{:?}", a.run.controls.kept.change);
            // The history: A noted the handover (no coordinator here: on the NAS, kept for its next).
            let notes = std::fs::read_to_string(a.home.join("pool").join(NOTES)).unwrap();
            assert!(notes.contains("term 2: handed over by") && notes.contains("stepped down from term 1"), "{notes}");
            // And back: "Hand the Build To ▸ A" on the lead, B.
            until(&mut [&mut a, &mut b], |m| m[1].view().members.iter().any(|x| x.member == ia && x.can_lead));
            b.ask(LeadAsk::Give { to: ia.clone() });
            b.go();
            assert_eq!(b.asked().state, State::Going);
            until(&mut [&mut a, &mut b], |m| m[1].asked().state == State::Done);
            until(&mut [&mut a, &mut b], |m| m[0].run.side.driver().leads() == Some(3));
            assert!(a.view().leading && a.view().members.iter().any(|m| m.member == ib && m.can_lead));
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
            assert!(!o.events.iter().any(|e| matches!(e, Event::Handover { .. })) && a.run.side.driver().leads() == Some(1));
            // B's ask to take over, unforced, the lead in touch: refused, saying force is needed.
            b.ask(LeadAsk::Take { force: false, downgrade: false });
            b.go();
            assert!(b.asked().state == State::Refused && b.asked().said.contains("needs the owner's force"), "{:?}", b.asked());
            assert_eq!(b.run.side.driver().leads(), None);
        }

        #[test]
        fn a_handover_whose_target_doesnt_answer_is_given_up() {
            let d = tempfile::tempdir().unwrap();
            let (mut a, b) = pool(d.path());
            a.ask(LeadAsk::Give { to: b.id() });
            let o = a.go();
            assert!(o.events.iter().any(|e| matches!(e, Event::Handover { what: "offered", .. })), "{:?}", o.events);
            assert_eq!(a.asked().state, State::Going);
            assert!(a.view().handing.as_ref().is_some_and(|h| h.stage == Stage::Offered && h.to == b.id()), "{:?}", a.view().handing);
            // B never answers: a minute on, the offer is given up; A leads term 1 still.
            a.pass(61);
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
            // A is silent; eleven minutes on B's clock.
            b.pass(660);
            b.go();
            let v = b.view();
            assert!(v.no_lead.as_ref().is_some_and(|w| w.contains("out of touch")), "{v:?}");
            assert_eq!(v.takeover, Some(Takeover::default()), "no force needed");
            b.ask(LeadAsk::Take { force: false, downgrade: false });
            b.go();
            until(&mut [&mut b], |m| m[0].asked().state == State::Done);
            assert_eq!(b.run.side.driver().leads(), Some(2));
            let term2: crate::pool::term::Term = serde_json::from_slice(&std::fs::read(b.root.join("state/build/terms/2.json")).unwrap()).unwrap();
            assert!(term2.how.starts_with("taken over by"), "{term2:?}");
            assert!(b.run.controls.kept.change.as_ref().is_some_and(|c| c.said.contains("This Mac leads term 2")));
            drop(a);
        }

        #[test]
        fn the_offer_on_battery_taken_by_itself_once_switched_on() {
            let d = tempfile::tempdir().unwrap();
            let (mut a, mut b) = pool(d.path());
            let ib = b.id();
            // Half an hour on since term 1 began; A on battery: B is offered, on every Mac's view.
            a.pass(CALM_S);
            b.pass(CALM_S);
            a.conds = Conds { ac: false, battery: Some(40), ..HOME };
            until(&mut [&mut a, &mut b], |m| m[1].view().offer.is_some());
            let o = b.view().offer.unwrap();
            assert_eq!((o.to.as_str(), o.why.ends_with("is on battery")), (ib.as_str(), true));
            // The switch off: five minutes on, nothing.
            a.pass(AUTO_AFTER_S + 30);
            b.pass(AUTO_AFTER_S + 30);
            for _ in 0..3 {
                a.go();
                b.go();
            }
            assert_eq!(a.run.side.driver().leads(), Some(1));
            // On: once the offer stood five minutes, the lead hands over by itself.
            std::fs::write(a.root.join(AUTO), "").unwrap();
            a.pass(VIEW_S);
            b.pass(VIEW_S);
            a.go();
            assert!(a.view().auto, "read with the view, every half minute");
            a.pass(AUTO_AFTER_S);
            b.pass(AUTO_AFTER_S);
            until(&mut [&mut a, &mut b], |m| m[1].run.side.driver().leads() == Some(2));
            assert!(a.asked().by.starts_with("the pool by itself"), "{:?}", a.asked());
            // B, its new lead, at once on battery too: no offer back for half an hour.
            b.conds = Conds { ac: false, battery: Some(40), ..HOME };
            a.conds = HOME;
            a.go();
            b.go();
            a.pass(VIEW_S);
            b.pass(VIEW_S);
            until(&mut [&mut a, &mut b], |m| m[1].view().offer.is_some());
            b.pass(AUTO_AFTER_S + 60);
            a.pass(AUTO_AFTER_S + 60);
            for _ in 0..4 {
                a.go();
                b.go();
            }
            assert_eq!(b.run.side.driver().leads(), Some(2), "no flapping back");
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
    }
}
