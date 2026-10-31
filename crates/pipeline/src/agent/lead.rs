//! What every view shows of the pool's lead (docs/pool.md §6, §10): the lead, which is fixed (the
//! member `state/build/lead-member.json` declares: the Mac mini; no election, handover or takeover,
//! task #881), its term, and the members.
//!
//! - **The view** (`View`, in the agent's status): the declared lead and its term; each member,
//!   read by id from its heartbeat (never listed), with its state; "no lead", and why (the lead out
//!   of touch: the members finish their jobs and wait); the last change of term in words. Made
//!   again every `VIEW_S`, and at once after a change.
//! - **The history** (`notes`): each term made (term 1, the lead's re-assertions) and stepped down
//!   from, as `term` events: into the lead's coordinator's history (the build page's activity, and
//!   the NAS's history per writer through it); a member, without one, appends them to its own file
//!   on the NAS and keeps them for its next process's coordinator (`NOTES`).

use super::pool::{Heartbeat, Run};
use crate::coord::history;
use crate::pool::driver::{Event, Out};
use crate::pool::{reachable, term};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// No lead for this long (by this Mac's clock: since the lead was last heard from, or since this
/// Mac first saw no lead in the term) is an alert, on every view (s).
pub const ALERT_S: u64 = 3600;
/// The view is made again this often (s): a heartbeat read per member.
const VIEW_S: u64 = 30;
/// The terms' events a member keeps for its next process's coordinator, in the agent's pool folder.
pub const NOTES: &str = "notes.jsonl";
/// What it keeps across its processes, in the agent's pool folder.
const KEPT: &str = "lead.json";

/// The last change of term this Mac saw, in words ("No longer leading …").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub at: u64,
    pub said: String,
}

/// What it keeps across its processes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Kept {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<Change>,
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
    /// In words: home on power, on battery (54 %), away, out of touch, clock wrong, app too old.
    pub state: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub out_of_touch: bool,
    /// Away from home.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub away: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conds: Option<super::pool::Conds>,
    /// Another member in touch reports it can't reach its coordinator (docs/pool.md §6.8), in
    /// words: "unreachable from m4: connection timed out (…)".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unreached: Option<String>,
}

/// The pool, as the views show it (in the agent's status: `PoolView::lead`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct View {
    /// When it was made (this Mac's clock).
    pub at: u64,
    pub term: u64,
    /// The current term's lead (None: the term can't be read whole).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lead: Option<LeadOf>,
    /// The declared lead's member id (crate::pool::term::declared; None: none declared yet).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared: Option<String>,
    /// This Mac leads it.
    pub leading: bool,
    /// Every member it knows, this Mac's first, then by host.
    pub members: Vec<Member>,
    /// No lead anyone can reach, and why: the lead out of touch, or the term unreadable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_lead: Option<String>,
    /// No lead for `ALERT_S` or more: since when, why, and what to do, in words (the menu bar's
    /// icon and the build page show it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alert: Option<String>,
    /// The term's lead can't take it up: the journal GC's marker refuses its records (§7.3), for so
    /// long by its clock (s), and why. An alert once `ALERT_S` (a stale read passes sooner).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fenced: Option<(u64, String)>,
    /// Who can't reach whom (docs/pool.md §6.8), each in words with what the owner can do: "m1's
    /// coordinator is unreachable from m4: connection timed out (…)".
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unreached: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<Change>,
}

/// The views' part of the agent's pool, in this process.
#[derive(Debug, Default)]
pub struct Controls {
    pub kept: Kept,
    kept_written: Option<Kept>,
    pub view: Option<View>,
    /// The view is made again at the next loop (a change).
    pub stale: bool,
}

impl Controls {
    /// What it kept in the agent's pool folder `dir` (an older app's asks there, dropped).
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
            .and_then(|b| store::whole::write(&dir.join(KEPT), &b));
        match r {
            Ok(()) => self.kept_written = Some(self.kept.clone()),
            Err(e) => eprintln!("pool: keeping the lead's view state: {e:#}"),
        }
    }
}

/// A member named `name` (its id, or its host name, any case): its id; or why there's none (a pause
/// of one member, crate::agent).
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

/// After a step: the last change of term noted, the history's terms written, the view made again
/// when due. `coord`: this process's coordinator (the lead's), whose history the terms go to.
pub fn after(
    run: &mut Run,
    out: &Out,
    root: &store::nasfs::NasRoot,
    home: &Path,
    coord: Option<&crate::coord::Coordinator>,
) {
    let now = run.side.now();
    let dir = home.join("pool");
    let me = run.side.member().id.clone();
    // The history.
    let notes = notes(&out.events, &run.side.member().host, now);
    if !notes.is_empty() {
        note(&notes, coord, root, &me, &dir);
        run.controls.stale = true;
    }
    let leading = run.side.driver().current().lead.as_ref().is_some_and(|t| t.member == me);
    let host = run.side.member().host.clone();
    let joins = membership(&mut run.in_touch, &run.side.beats, run.side.beats_at, &me, &host, leading);
    if !joins.is_empty() {
        note(&joins, coord, root, &me, &dir);
        run.controls.stale = true;
    }
    // The last change of term, in words.
    let cur = run.side.driver().current().lead.clone();
    for e in &out.events {
        if let Event::SteppedDown { term, why } = e {
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
    if out.events.iter().any(|e| matches!(e, Event::Made { .. } | Event::SteppedDown { .. } | Event::TookUp { .. })) {
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
    run.controls.save(&dir);
}

/// A member's state in words (§11), at the reader's `now`, against the app the records need `app`.
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
    if app.is_some_and(|a| !term::app_at_least(&b.app, a)) {
        return "app too old".into();
    }
    match h.conds {
        None => "conditions unknown (an older app)".into(),
        Some(c) if c.held => "paused".into(),
        Some(c) if !c.home => "away".into(),
        Some(c) if !c.ac => c.battery.map_or("on battery".to_string(), |p| format!("on battery ({p} %)")),
        Some(_) => "home on power".into(),
    }
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
    let regions = side.regions_app();
    let need = crate::inputs::regions::read::needed_app(
        cur.lead.as_ref().map(|t| t.app.as_str()),
        regions.as_ref().ok().and_then(|r| r.as_deref()),
    );
    for id in ids {
        let h = side.heartbeat(&id).unwrap_or_default();
        // (Not a shadow run's heartbeat.)
        if h.shadow {
            continue;
        }
        let is_lead = cur.lead.as_ref().is_some_and(|t| t.member == id);
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
            conds: h.conds,
            unreached,
        });
        beats.push((id, h));
    }
    members.sort_by(|a, b| b.me.cmp(&a.me).then(b.leads.cmp(&a.leads)).then(a.host.cmp(&b.host)));
    let lead_beat =
        cur.lead.as_ref().and_then(|t| beats.iter().find(|(id, _)| *id == t.member)).map(|(_, h)| h.clone());
    let no_lead = match (&cur.lead, &lead_beat) {
        _ if cur.term == 0 => None,
        (None, _) => Some(format!("term {} can't be read whole", cur.term)),
        (Some(t), _) if leading || t.member == me => None,
        (Some(t), None) => Some(format!("no heartbeat from {}", t.host)),
        (Some(t), Some(h)) if h.pool.beat == 0 => Some(format!("no heartbeat from {}", t.host)),
        (Some(t), Some(h)) if h.pool.out_of_touch(now) => {
            Some(format!("{} is out of touch: last heard from {}", t.host, ago(now, h.pool.beat)))
        }
        _ => None,
    };
    let fenced = lead_beat
        .as_ref()
        .filter(|h| !h.pool.out_of_touch(now))
        .and_then(|h| h.pool.fenced.as_ref().map(|f| (h.pool.beat.saturating_sub(f.since), f.why.clone())));
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
        declared: side.driver().declared().map(str::to_string),
        leading,
        members,
        no_lead,
        alert: None,
        fenced,
        unreached,
        change: run.controls.kept.change.clone(),
    }
}

/// The alert (`View::alert`): no lead for `ALERT_S` or more, since the lead was last heard from or
/// since this Mac first saw no lead in this term (`kept.no_lead`), whichever is earlier; with why,
/// and what to do: the lead is fixed, so it's the lead Mac to see to (no other Mac takes over).
pub fn alert(v: &mut View, kept: &mut Kept, now: u64) {
    // (A lead that can't take its term up for the journal GC's marker: no lead all the same; the
    // marker lifted is the way on.)
    if let (Some((for_s, why)), Some(l)) = (&v.fenced, &v.lead) {
        if *for_s >= ALERT_S {
            v.alert = Some(format!(
                "No lead for {}: {} can't take up term {}: {why}. Nothing builds until it can: run `scenic pool \
                 journal-gc` on any Mac to see why and whether it's safe to lift (docs/pool.md §7.3).",
                span(*for_s),
                l.host,
                v.term
            ));
            return;
        }
    }
    let Some(why) = v.no_lead.clone() else {
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
    let host = v.lead.as_ref().map_or("the lead".to_string(), |l| l.host.clone());
    v.alert = Some(format!(
        "No lead for {}: {why}. Nothing new builds until {host} is back (the members finish their jobs and wait): \
         check it is awake, on the network and its agent running.",
        span(now.saturating_sub(since))
    ));
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
pub const OWNER_STEPS: &str = "On the Mac named: check it is awake, on the network and its agent running; then allow Scenic in System Settings › Network › Firewall (Options…), or turn the firewall off";

/// The step's events of the terms, as the history notes them (kind `term`, `worker` this Mac's
/// host): each term made (its `how`) and stepped down from.
pub fn notes(events: &[Event], me: &str, now: u64) -> Vec<history::Event> {
    let mut out = Vec::new();
    for e in events {
        let note = match e {
            Event::Made { term, how } => format!("term {term}: {how}"),
            Event::SteppedDown { term, why } => format!("stepped down from term {term}: {why}"),
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

/// The members coming into touch and going out of it, as the lead sees them (docs/pool.md §10,
/// History; kind `member`, `worker` this Mac's host): its members' heartbeats as last read whole
/// (`beats`, read at `at`), each in touch while its beat is under `OUT_OF_TOUCH_S` old, compared
/// with the last read's (`kept`: their ids and hosts). Only while this Mac leads (`leading`), and
/// once a read: a lead's first read only sets what it compares with, so a new lead or a restart
/// notes nothing for the members already there.
pub fn membership(
    kept: &mut Option<(u64, BTreeMap<String, String>)>,
    beats: &[crate::pool::beat::Beat],
    at: Option<u64>,
    me: &str,
    host: &str,
    leading: bool,
) -> Vec<history::Event> {
    let (Some(at), true) = (at, leading) else {
        if !leading {
            *kept = None;
        }
        return Vec::new();
    };
    if kept.as_ref().is_some_and(|(t, _)| *t == at) {
        return Vec::new();
    }
    let now: BTreeMap<String, String> = beats
        .iter()
        .filter(|b| b.member != me && b.beat > 0 && !b.out_of_touch(at))
        .map(|b| (b.member.clone(), if b.host.is_empty() { b.member.clone() } else { b.host.clone() }))
        .collect();
    let Some((_, was)) = kept.replace((at, now.clone())) else { return Vec::new() };
    let ev = |note: String| history::Event {
        t: at,
        kind: "member".into(),
        worker: Some(host.to_string()),
        note,
        ..Default::default()
    };
    let mut out: Vec<history::Event> =
        now.iter().filter(|(id, _)| !was.contains_key(*id)).map(|(_, h)| ev(format!("{h} joined the pool"))).collect();
    out.extend(
        was.iter()
            .filter(|(id, _)| !now.contains_key(*id))
            .map(|(_, h)| ev(format!("{h} left the pool (out of touch)"))),
    );
    out
}

/// The terms' events noted: appended to this member's history on the NAS (`root`) at once (a lead
/// stepping down appends its coordinator's history no more: crate::agent's `pool_lead_files`
/// leaves them out); and into this process's coordinator's history, when it has one, else kept in
/// its pool folder `dir` for its next process's coordinator (`replay`).
fn note(
    events: &[history::Event],
    coord: Option<&crate::coord::Coordinator>,
    root: &store::nasfs::NasRoot,
    member: &str,
    dir: &Path,
) {
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
    if let (Some((for_s, why)), Some(l)) = (&v.fenced, &v.lead) {
        out.push(
            v.alert
                .clone()
                .unwrap_or_else(|| format!("{} can't take up term {} for {}: {why}", l.host, v.term, span(*for_s))),
        );
    }
    match (&v.lead, &v.no_lead) {
        (_, Some(why)) => out.push(v.alert.clone().unwrap_or_else(|| format!("No lead: {why}"))),
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
    if let (Some(d), Some(l)) = (&v.declared, &v.lead) {
        if *d != l.member {
            let host = v.members.iter().find(|m| m.member == *d).map_or(d.clone(), |m| m.host.clone());
            out.push(format!("The declared lead is {host}: it takes the next term"));
        }
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
        out.push(format!(
            "  {}{tags}: {}{heard}, app {}",
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
    if let Some(c) = &v.change {
        out.push(format!("{} ({})", c.said, ago(now, c.at)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::pool::Conds;
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

    const HOME: Conds = Conds { home: true, ac: true, battery: Some(90), held: false };

    #[test]
    fn a_lead_fenced_by_the_journal_gcs_marker_for_an_hour_is_an_alert_saying_what_to_do() {
        // (Task #651) The lead's heartbeat says its take-ups are refused by the marker: a stale
        // read passes within the hour, so a line in `scenic lead` until then; then the alert, which
        // every view (the build page, the menu bar) shows.
        let mut kept = Kept::default();
        let why = "the journal GC's marker (state/journal/gc.json) can't be parsed (EOF)";
        let mut v = View {
            term: 9,
            lead: Some(LeadOf { term: 9, member: "m-a".into(), host: "Mac-mini".into(), ..Default::default() }),
            fenced: Some((ALERT_S - 1, why.into())),
            ..Default::default()
        };
        alert(&mut v, &mut kept, 5000);
        assert_eq!(v.alert, None);
        assert!(
            said(&v, 5000)[0].starts_with("Mac-mini can't take up term 9 for 59 min: the journal GC"),
            "{:?}",
            said(&v, 5000)
        );
        v.fenced = Some((2 * 3600, why.into()));
        alert(&mut v, &mut kept, 5000);
        assert_eq!(
            v.alert.as_deref(),
            Some("No lead for 2 h: Mac-mini can't take up term 9: the journal GC's marker (state/journal/gc.json) can't be parsed (EOF). Nothing builds until it can: run `scenic pool journal-gc` on any Mac to see why and whether it's safe to lift (docs/pool.md §7.3).")
        );
        assert_eq!(said(&v, 5000)[0], v.alert.clone().unwrap());
    }

    #[test]
    fn the_alert_counts_from_the_first_sight_of_no_lead_or_the_leads_last_beat() {
        let mut kept = Kept::default();
        // Term 55 unreadable: seen at 1000, an alert from an hour on.
        let mut v = View { term: 55, no_lead: Some("term 55 can't be read whole".into()), ..Default::default() };
        alert(&mut v, &mut kept, 1000);
        assert_eq!((v.alert.as_deref(), kept.no_lead), (None, Some((55, 1000))));
        alert(&mut v, &mut kept, 1000 + ALERT_S - 1);
        assert_eq!(v.alert, None);
        alert(&mut v, &mut kept, 1000 + 3 * 3600);
        assert!(v.alert.as_deref().is_some_and(|w| w.starts_with("No lead for 3 h: term 55 can't be read whole. ")));
        // The lead out of touch: from its last beat, though first seen just now; no takeover offered,
        // the lead Mac to see to.
        let lead = LeadOf { term: 56, member: "m-a".into(), host: "Mac-mini".into(), ..Default::default() };
        let mut v = View {
            term: 56,
            lead: Some(lead),
            members: vec![Member { member: "m-a".into(), beat: 30_000, ..Default::default() }],
            no_lead: Some("Mac-mini is out of touch".into()),
            ..Default::default()
        };
        kept.no_lead = None;
        alert(&mut v, &mut kept, 30_000 + ALERT_S + 120);
        assert_eq!(
            v.alert.as_deref(),
            Some("No lead for 62 min: Mac-mini is out of touch. Nothing new builds until Mac-mini is back (the members finish their jobs and wait): check it is awake, on the network and its agent running.")
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

    #[test]
    fn the_terms_events_in_the_history() {
        let events = [
            Event::Made { term: 4, how: "restarted: re-asserted".into() },
            Event::SteppedDown { term: 3, why: "another member is the declared lead".into() },
            Event::Merged { applied: vec![], overtaken: 0, refused: 0, listed: false, done: vec![] },
        ];
        let n = notes(&events, "mini", 5);
        let said: Vec<&str> = n.iter().map(|e| e.note.as_str()).collect();
        assert_eq!(
            said,
            ["term 4: restarted: re-asserted", "stepped down from term 3: another member is the declared lead"]
        );
        assert!(n.iter().all(|e| e.kind == "term" && e.worker.as_deref() == Some("mini") && e.t == 5));
    }

    #[test]
    fn members_joining_and_leaving_in_the_history() {
        let beat = |m: &str, h: &str, at: u64| crate::pool::beat::Beat {
            member: m.into(),
            host: h.into(),
            beat: at,
            ..serde_json::from_str(r#"{"member": "x", "beat": 1}"#).unwrap()
        };
        let notes = |e: Vec<history::Event>| e.into_iter().map(|e| (e.kind, e.note)).collect::<Vec<_>>();
        let mut k = None;
        let t0 = 10_000;
        let two = [beat("m-me", "mini", t0), beat("m-a", "air", t0)];
        // A lead's first read sets what it compares with; reading it again, or not leading, notes nothing.
        assert!(membership(&mut k, &two, Some(t0), "m-me", "mini", true).is_empty());
        assert!(membership(&mut k, &two, Some(t0), "m-me", "mini", true).is_empty());
        let three = [beat("m-me", "mini", t0 + 60), beat("m-a", "air", t0 + 60), beat("m-b", "", t0 + 60)];
        let n = notes(membership(&mut k, &three, Some(t0 + 60), "m-me", "mini", true));
        assert_eq!(n, [("member".to_string(), "m-b joined the pool".to_string())]);
        // Air's beat grows old: it left.
        let t1 = t0 + 60 + crate::pool::beat::OUT_OF_TOUCH_S + 1;
        let late = [beat("m-me", "mini", t1), beat("m-a", "air", t0 + 60), beat("m-b", "", t1)];
        assert_eq!(
            notes(membership(&mut k, &late, Some(t1), "m-me", "mini", true)),
            [("member".to_string(), "air left the pool (out of touch)".to_string())]
        );
        // Not leading: forgotten, and the next lead's first read notes nothing.
        assert!(membership(&mut k, &two, Some(t1 + 60), "m-me", "mini", false).is_empty());
        assert!(k.is_none());
        assert!(membership(&mut k, &two, Some(t1 + 120), "m-me", "mini", true).is_empty());
    }

    /// Two members of a pool in one process, A (this Mac, the declared lead: term 1 is its) and B,
    /// each with its folder, stepped as the agent steps them (`go`).
    mod runs {
        use super::super::*;
        use crate::agent::pool::{Conds, Gates, Role, Run, SharedNas, Side};
        use std::sync::Arc;

        const HOME: Conds = Conds { home: true, ac: true, battery: None, held: false };

        pub(super) struct Mac {
            pub conds: Conds,
            pub run: Run,
            pub home: PathBuf,
            pub root: store::nasfs::NasRoot,
        }

        pub(super) fn pool(d: &Path) -> (Mac, Mac) {
            let r = d.join("nas");
            std::fs::create_dir_all(r.join("state/build")).unwrap();
            std::fs::write(r.join("state/build/manifest.json"), "{}").unwrap();
            std::fs::write(r.join("state/build/jobs.json"), "{}").unwrap();
            std::fs::write(r.join("state/build/pending.json"), "{}").unwrap();
            let mac = |n: &str| {
                let home = d.join(n).join("agent");
                let nas: SharedNas = Arc::new(store::nasfs::NasRoot::local_for_test(&r));
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
                Mac { conds: HOME, run, home, root: store::nasfs::NasRoot::local_for_test(r.clone()) }
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
            /// One loop's part: the step, what came of it.
            pub fn go(&mut self) -> Out {
                self.run.conds = Some(self.conds);
                self.run.tend_reach();
                let out = self.run.step();
                after(&mut self.run, &out, &self.root, &self.home, None);
                out
            }
            pub fn id(&self) -> String {
                self.run.side.member().id.clone()
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
            let end = std::time::Instant::now() + store::testkit::WATCHDOG;
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
                    macs.iter().map(|m| m.run.side.driver().current().clone()).collect::<Vec<_>>()
                );
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
        }

        /// The rollout of the records' readers (docs/pool.md §6.2): the lead of a later term writes
        /// today's files while any member's heartbeat says its app reads them (an older app's,
        /// without `reads_records`), and no more once every member's reads the term's snapshot;
        /// this app's readers read the snapshot either way.
        #[test]
        fn todays_files_kept_while_a_members_app_reads_them() {
            use crate::pool::reachable::PROBE_S;
            let d = tempfile::tempdir().unwrap();
            let (mut a, mut b) = pool(d.path());
            // A restarts: it re-asserts, term 2 (a later term's lead writes today's files).
            a.run.side.restart().unwrap();
            until(&mut [&mut a, &mut b], |m| {
                m[0].run.side.driver().records().is_some_and(|r| r.term == 2 && r.seq > 0)
            });
            let root = a.root.clone();
            let mf = root.join("state/build/manifest.json").unchecked_path();
            let read = |a: &Mac| crate::out::records_now(&a.root).unwrap();
            let rec = a.run.side.driver().records().unwrap().clone();
            assert_eq!((read(&a).term, read(&a).seq), (rec.term, rec.seq), "this app's readers: the term's snapshot");
            // Both Macs on this app: today's files written no more.
            pass_all(&mut [&mut b, &mut a], PROBE_S);
            a.go();
            assert!(a.run.side.readers_ready, "both heartbeats say so: {:?}", a.run.side.beats);
            std::fs::remove_file(&mf).unwrap();
            a.run.keep_today(true);
            assert!(!mf.exists(), "every member reads the snapshot: not written");
            // B's heartbeat not there (lost, or B never beat): counted as an older app's.
            let hb = root.join(crate::pool::beat::path(&b.id())).unchecked_path();
            let kept = std::fs::read(&hb).unwrap();
            std::fs::remove_file(&hb).unwrap();
            a.pass(PROBE_S);
            a.go();
            assert!(!a.run.side.readers_ready, "a member without a heartbeat");
            std::fs::write(&hb, &kept).unwrap();
            // B on an older app: its heartbeat without the field (B not stepped, so it stays so).
            let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&hb).unwrap()).unwrap();
            assert_eq!(v.as_object_mut().unwrap().remove("reads_records"), Some(serde_json::Value::Bool(true)));
            std::fs::write(&hb, serde_json::to_vec(&v).unwrap()).unwrap();
            a.pass(PROBE_S);
            a.go();
            assert!(!a.run.side.readers_ready, "B's app reads today's files");
            a.run.keep_today(true);
            let m: BTreeMap<String, String> =
                crate::out::read_record(&store::nasfs::NasPath::local_for_test(&mf)).unwrap();
            assert_eq!(m, a.run.side.driver().records().unwrap().manifest, "written for B's readers");
            assert_eq!(a.run.today.map(|t| t.0), Some(2));
            // B back on this app: written no more.
            pass_all(&mut [&mut b, &mut a], crate::pool::beat::BEAT_EVERY_S + PROBE_S);
            a.go();
            assert!(a.run.side.readers_ready);
            std::fs::remove_file(&mf).unwrap();
            a.run.keep_today(true);
            assert!(!mf.exists() && a.run.today.is_none());
        }

        /// Task #881: the mini is the fixed lead. A term naming a member (an elected lead's, from
        /// before) isn't led by it: the declared lead makes the next. With the lead gone, the member
        /// makes no term and leads nothing, however long; every view says so.
        #[test]
        fn a_member_never_claims_the_lead_and_waits_for_it() {
            let d = tempfile::tempdir().unwrap();
            let (mut a, mut b) = pool(d.path());
            let (ia, ib) = (a.id(), b.id());
            let declared = crate::pool::term::declared(a.run.side.nas()).unwrap().map(|d| d.member);
            assert_eq!(declared, Some(ia.clone()), "term 1's maker declared itself");
            let t2 = crate::pool::term::Term {
                term: 2,
                member: ib.clone(),
                host: "b".into(),
                app: "development".into(),
                since: b.run.side.now(),
                how: "taken over by b".into(),
                from: 1,
            };
            b.run.side.nas().create_new(&crate::pool::term::path(2), &serde_json::to_vec(&t2).unwrap()).unwrap();
            for _ in 0..3 {
                assert_eq!(b.go().leads, None, "B never leads");
            }
            until(&mut [&mut a, &mut b], |m| m[0].run.side.driver().leads() == Some(3));
            let t3 = b.run.side.driver().current().lead.clone().unwrap();
            assert_eq!((t3.member.as_str(), t3.how.as_str()), (ia.as_str(), "the declared lead, after b"));
            // A gone for two hours: B makes no term and leads nothing; its view says to see to A.
            b.go();
            b.pass(2 * 3600);
            assert_eq!(b.go().leads, None);
            assert_eq!(b.run.side.driver().current().term, 3);
            assert!(!d.path().join("nas").join(crate::pool::term::path(4)).exists(), "no term made");
            let v = b.view();
            assert!(v.no_lead.as_deref().is_some_and(|w| w.contains("out of touch")), "{v:?}");
            assert!(v.alert.as_deref().is_some_and(|w| w.contains("the members finish their jobs and wait")), "{v:?}");
            assert!(said(&v, b.run.side.now())[0] == v.alert.clone().unwrap());
            drop(a);
        }
    }
}
