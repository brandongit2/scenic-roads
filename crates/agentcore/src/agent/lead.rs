//! What every view shows of the pool's lead (docs/pool.md §6, §10): the lead, which is fixed (the
//! member `state/build/lead-member.json` declares: the Mac mini; no election, handover or takeover,
//! task #881; no terms, docs/coordination.md step 8c), and the members.
//!
//! - **The view** (`View`, in the agent's status): the declared lead; each member, read by id from
//!   its heartbeat (never listed), with its state; "no lead", and why (the lead out of touch: the
//!   members finish their jobs and wait); the last change of lead in words. Made again every
//!   `VIEW_S`, and at once after a change.
//! - **The history** (`notes`): each take-up of the records and each step down, as `term` events
//!   (their kind's name from when the lead's runs were terms): into the lead's coordinator's history
//!   (the build page's activity, and the NAS's history per writer through it); a member, without
//!   one, appends them to its own file on the NAS and keeps them for its next process's coordinator
//!   (`NOTES`).

use super::pool::{Heartbeat, Run};
use crate::coord::history;
use crate::pool::driver::{Event, Out};
use crate::pool::{lead, reachable};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// No lead for this long (by this Mac's clock: since the lead was last heard from, or since this
/// Mac first saw no lead) is an alert, on every view (s).
pub const ALERT_S: u64 = 3600;
/// The view is made again this often (s): a heartbeat read per member.
const VIEW_S: u64 = 30;
/// The lead's events a member keeps for its next process's coordinator, in the agent's pool folder.
pub const NOTES: &str = "notes.jsonl";
/// What it keeps across its processes, in the agent's pool folder.
const KEPT: &str = "lead.json";

/// The last change of lead this Mac saw, in words ("No longer leading …").
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
    /// Since when (this Mac's clock) no lead anyone can reach has been seen: for the alert
    /// (`ALERT_S`). (An app before docs/coordination.md step 8c kept a term with it: read as none.)
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "since_or_none")]
    pub no_lead: Option<u64>,
}

/// `Kept::no_lead` as kept, or as an app before step 8c kept it (a term and since when): none.
fn since_or_none<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    Ok(Option::<serde_json::Value>::deserialize(d)?.and_then(|v| v.as_u64()))
}

/// The lead, as this Mac knows it: its declaration's member, its host and app (empty while no answer
/// of it came lately), and when it took the records up (this Mac's own lead; 0 for another's).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeadOf {
    pub member: String,
    pub host: String,
    pub app: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub since: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
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
    /// It's the declared lead.
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
    /// The declared lead (None: none declared yet, or the declaration not read).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lead: Option<LeadOf>,
    /// The declared lead's member id (crate::pool::lead::declared; None: none declared yet).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared: Option<String>,
    /// This Mac leads it.
    pub leading: bool,
    /// Every member it knows, this Mac's first, then by host.
    pub members: Vec<Member>,
    /// No lead anyone can reach, and why: the lead out of touch, or none declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_lead: Option<String>,
    /// No lead for `ALERT_S` or more: since when, why, and what to do, in words (the menu bar's
    /// icon and the build page show it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alert: Option<String>,
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

/// After a step: the last change of lead noted, the history's take-ups written, the view made again
/// when due. `coord`: this process's coordinator (the lead's), whose history they go to.
pub fn after(run: &mut Run, out: &Out, home: &Path, coord: Option<&crate::coord::Coordinator>) {
    let now = run.side.now();
    let dir = home.join("pool");
    let me = run.side.member().id.clone();
    // The lead it is, for its members' beats' answers (docs/coordination.md steps 8a, 8c).
    if let Some(c) = coord {
        c.set_led(out.leads.map(|e| (e, run.side.member().clone())));
        // The records' brief it serves (docs/coordination.md step 8d): as last saved whole, on its
        // disk and the NAS's one path; none once it no longer leads.
        let d = run.side.driver();
        match d.records() {
            Some(r) if d.saved_records() == Some((r.epoch, r.seq)) => c.set_brief(Some(r)),
            Some(_) => {}
            None => c.set_brief(None),
        }
    }
    // The history.
    let notes = notes(&out.events, &run.side.member().host, now);
    if !notes.is_empty() {
        note(&notes, coord, &dir);
        run.controls.stale = true;
    }
    let leading = run.side.driver().declared() == Some(me.as_str());
    let host = run.side.member().host.clone();
    let joins = membership(&mut run.in_touch, &run.side.beats, run.side.beats_at, &me, &host, leading);
    if !joins.is_empty() {
        note(&joins, coord, &dir);
        run.controls.stale = true;
    }
    // The last change of lead, in words.
    let next = run.side.driver().lead().filter(|l| l.id != me).map(|l| l.host.clone());
    for e in &out.events {
        match e {
            Event::SteppedDown { why, .. } => {
                let next = next.as_ref().map(|h| format!("; {h} is the declared lead")).unwrap_or_default();
                run.controls.kept.change = Some(Change { at: now, said: format!("No longer leading: {why}{next}") });
            }
            Event::TookUp { how, .. } => {
                run.controls.kept.change =
                    Some(Change { at: now, said: format!("This Mac leads (took the records up from {how})") });
            }
            _ => continue,
        }
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
    if app.is_some_and(|a| !lead::app_at_least(&b.app, a)) {
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
    let leading = side.driver().leads().is_some();
    let lead = side.driver().lead().map(|l| LeadOf {
        member: l.id.clone(),
        host: l.host.clone(),
        app: l.app.clone(),
        since: if l.id == me { side.driver().leads_since().unwrap_or(0) } else { 0 },
    });
    let mut ids: Vec<String> = side.members().iter().cloned().collect();
    if let Some(l) = &lead {
        if !ids.contains(&l.member) {
            ids.push(l.member.clone());
        }
    }
    let pool_beats: Vec<crate::pool::Beat> = ids.iter().filter_map(|id| side.heartbeat(id)).map(|h| h.pool).collect();
    let mut beats: Vec<(String, Heartbeat)> = Vec::new();
    let mut members: Vec<Member> = Vec::new();
    // (The app a member needs: the lead's, and once a readers' switch is recorded, the one it was
    // recorded on, whichever is newer: an older one is "app too old".)
    let regions = run.brief().map_err(|e| format!("the records: {e:#}")).and_then(|b| side.regions_app(&b.manifest));
    let lead_app = lead.as_ref().map(|l| l.app.as_str()).filter(|a| !a.is_empty());
    let need = inputs::inputs::regions::read::needed_app(lead_app, regions.as_ref().ok().and_then(|r| r.as_deref()));
    for id in ids {
        let h = side.heartbeat(&id).unwrap_or_default();
        let is_lead = lead.as_ref().is_some_and(|l| l.member == id);
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
    let lead_beat = lead.as_ref().and_then(|l| beats.iter().find(|(id, _)| *id == l.member)).map(|(_, h)| h.clone());
    let no_lead = match (&lead, &lead_beat) {
        (None, _) => Some("no lead is declared".to_string()),
        (Some(l), _) if leading || l.member == me => None,
        (Some(l), None) => Some(format!("no heartbeat from {}", l.host)),
        (Some(l), Some(h)) if h.pool.beat == 0 => Some(format!("no heartbeat from {}", l.host)),
        (Some(l), Some(h)) if h.pool.out_of_touch(now) => {
            Some(format!("{} is out of touch: last heard from {}", l.host, ago(now, h.pool.beat)))
        }
        _ => None,
    };
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
        lead,
        declared: side.driver().declared().map(str::to_string),
        leading,
        members,
        no_lead,
        alert: None,
        unreached,
        change: run.controls.kept.change.clone(),
    }
}

/// The alert (`View::alert`): no lead for `ALERT_S` or more, since the lead was last heard from or
/// since this Mac first saw no lead (`kept.no_lead`), whichever is earlier; with why, and what to
/// do: the lead is fixed, so it's the lead Mac to see to (no other Mac takes over).
pub fn alert(v: &mut View, kept: &mut Kept, now: u64) {
    let Some(why) = v.no_lead.clone() else {
        kept.no_lead = None;
        v.alert = None;
        return;
    };
    let seen = *kept.no_lead.get_or_insert(now);
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

/// The step's events of the lead, as the history notes them (kind `term`, `worker` this Mac's
/// host): each take-up of the records (in its epoch, from where) and each step down.
pub fn notes(events: &[Event], me: &str, now: u64) -> Vec<history::Event> {
    let mut out = Vec::new();
    for e in events {
        let note = match e {
            Event::TookUp { epoch, how, .. } => format!("took the records up in epoch {epoch}, from {how}"),
            Event::SteppedDown { epoch, why } => format!("stepped down (epoch {epoch}): {why}"),
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

/// The lead's events noted: into this process's coordinator's history (its file on this Mac's
/// disk), when it has one, else kept in its pool folder `dir` for its next process's coordinator
/// (`replay`).
fn note(events: &[history::Event], coord: Option<&crate::coord::Coordinator>, dir: &Path) {
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
        eprintln!("pool: keeping the lead's events: {e}");
    }
}

/// The lead's events an earlier process of this member kept (`note`), into this process's
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
    match (&v.lead, &v.no_lead) {
        (_, Some(why)) => out.push(v.alert.clone().unwrap_or_else(|| format!("No lead: {why}"))),
        (Some(l), None) => out.push(format!(
            "The lead: {}{}{}",
            l.host,
            if v.leading { " (this Mac)" } else { "" },
            if l.since > 0 { format!(", since {}", ago(now, l.since)) } else { String::new() }
        )),
        (None, None) => out.push("No lead declared".into()),
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
    fn the_alert_counts_from_the_first_sight_of_no_lead_or_the_leads_last_beat() {
        let mut kept = Kept::default();
        // No lead declared: seen at 1000, an alert from an hour on.
        let mut v = View { no_lead: Some("no lead is declared".into()), ..Default::default() };
        alert(&mut v, &mut kept, 1000);
        assert_eq!((v.alert.as_deref(), kept.no_lead), (None, Some(1000)));
        alert(&mut v, &mut kept, 1000 + ALERT_S - 1);
        assert_eq!(v.alert, None);
        alert(&mut v, &mut kept, 1000 + 3 * 3600);
        assert!(v.alert.as_deref().is_some_and(|w| w.starts_with("No lead for 3 h: no lead is declared. ")));
        // The lead out of touch: from its last beat, though first seen just now; no takeover offered,
        // the lead Mac to see to.
        let lead = LeadOf { member: "m-a".into(), host: "Mac-mini".into(), ..Default::default() };
        let mut v = View {
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
        let mut v = View::default();
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
    fn the_leads_events_in_the_history() {
        let events = [
            Event::TookUp { epoch: 842, how: "its own records".into() },
            Event::SteppedDown { epoch: 842, why: "another member is the declared lead".into() },
            Event::Merged { applied: vec![], overtaken: 0, refused: 0, done: vec![], again: vec![] },
        ];
        let n = notes(&events, "mini", 5);
        let said: Vec<&str> = n.iter().map(|e| e.note.as_str()).collect();
        assert_eq!(
            said,
            [
                "took the records up in epoch 842, from its own records",
                "stepped down (epoch 842): another member is the declared lead"
            ]
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
                let mut run = Run::new(crate::agent::seam::for_tests(), side, Role::Member, Gates::default());
                // (Its tries of the others' coordinators in its step, answered at once: reached,
                // unless a test says otherwise.)
                run.prober.inline = true;
                run.prober.probe = Arc::new(|_: &[String]| Ok(()));
                Mac { conds: HOME, run, home }
            };
            let (mut a, mut b) = (mac("a"), mac("b"));
            assert_eq!(a.go().leads, Some(1));
            b.go();
            let (ia, ib) = (a.id(), b.id());
            a.run.side.know(&ib);
            b.run.side.know(&ia);
            hear(&mut [&mut a, &mut b]);
            (a, b)
        }

        /// Each Mac's heartbeat heard by the others, as their beats carry it (the lead's
        /// coordinator, and its answers to the members: crate::coord::beats), each by its own clock.
        pub(super) fn hear(macs: &mut [&mut Mac]) {
            let own: Vec<_> = macs.iter().filter_map(|m| m.run.side.http_beats().own()).collect();
            for m in macs.iter_mut() {
                let b = m.run.side.http_beats();
                for h in own.iter().filter(|h| h.pool.member != m.id()) {
                    let speaks =
                        [crate::coord::held::SPEAKS, crate::coord::beats::SPEAKS, crate::coord::beats::HANDOFF];
                    b.heard(h.clone(), speaks.map(str::to_string).to_vec(), None, m.run.side.now());
                }
            }
        }

        impl Mac {
            /// One loop's part: the step, what came of it.
            pub fn go(&mut self) -> Out {
                self.run.conds = Some(self.conds);
                self.run.tend_reach();
                let out = self.run.step();
                after(&mut self.run, &out, &self.home, None);
                out
            }
            pub fn id(&self) -> String {
                self.run.side.member().id.clone()
            }
            pub fn view(&self) -> View {
                self.run.controls.view.clone().expect("a view")
            }
            /// Both its clocks moved on `s` seconds, its run stepped every `TICK_S` meanwhile, as its
            /// pool's own thread steps it whatever its agent's loop does. The last step is the
            /// test's.
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

        /// docs/coordination.md steps 8a, 8c: the lead's agent tells its coordinator it leads after
        /// each step (`after`), and its beats' answers say it while it leads. Its declaration
        /// unreadable, it keeps its view and leads on; declared another (the owner moved the lead
        /// by hand), it leads nothing and says none.
        #[test]
        fn the_lead_s_answers_say_it_leads_while_it_leads() {
            let d = tempfile::tempdir().unwrap();
            let (mut a, b) = pool(d.path());
            let (c, _port) = crate::coord::start_for_test(&d.path().join("a/coord"), "a", "");
            let go = |a: &mut Mac| {
                a.run.conds = Some(a.conds);
                a.run.tend_reach();
                let out = a.run.step();
                after(&mut a.run, &out, &a.home, Some(&c));
                out
            };
            let said = || {
                let w = crate::coord::held::Beat {
                    member: "b".into(),
                    speaks: vec![crate::coord::held::SPEAKS.into()],
                    ..Default::default()
                };
                c.member_beat(&w).leading.map(|m| m.id)
            };
            (c.set_epoch(1), c.set_member(Some(&a.id())));
            let o = go(&mut a);
            assert_eq!(o.leads, Some(1), "{:?}", o.events);
            assert_eq!(said(), Some(a.id()));
            // Its declaration not readable (a share failing it): it leads on, and says so.
            let decl = d.path().join("nas").join(lead::LEAD);
            let kept = std::fs::read(&decl).unwrap();
            std::fs::remove_file(&decl).unwrap();
            std::fs::create_dir(&decl).unwrap();
            let o = go(&mut a);
            assert_eq!(o.leads, Some(1), "{:?}", o.events);
            assert_eq!(said(), Some(a.id()));
            std::fs::remove_dir(&decl).unwrap();
            std::fs::write(&decl, &kept).unwrap();
            // The owner declares B: A leads nothing, and says none.
            let to_b = lead::Declared { member: b.id(), host: "b".into() };
            std::fs::write(&decl, serde_json::to_vec(&to_b).unwrap()).unwrap();
            assert_eq!(go(&mut a).leads, None);
            assert_eq!(said(), None);
        }

        /// Task #881: the mini is the fixed lead. The member never leads; with the lead gone it
        /// leads nothing, however long, and every view says so (docs/coordination.md step 8c: no
        /// term is made, nor read).
        #[test]
        fn a_member_never_claims_the_lead_and_waits_for_it() {
            let d = tempfile::tempdir().unwrap();
            let (a, mut b) = pool(d.path());
            let ia = a.id();
            let declared = crate::pool::lead::declared(a.run.side.nas()).unwrap().map(|d| d.member);
            assert_eq!(declared, Some(ia.clone()), "the first Mac in the pool declared itself");
            for _ in 0..3 {
                assert_eq!(b.go().leads, None, "B never leads");
            }
            let v = b.view();
            assert_eq!(v.lead.as_ref().map(|l| l.member.as_str()), Some(ia.as_str()));
            assert!(!v.leading, "{v:?}");
            // A gone for two hours: B leads nothing; its view says to see to A.
            b.go();
            b.pass(2 * 3600);
            assert_eq!(b.go().leads, None);
            assert_eq!(b.run.side.driver().declared(), Some(ia.as_str()));
            assert!(!d.path().join("nas/state/build/terms").exists(), "no term made");
            let v = b.view();
            // (A's beat not heard for its 20 min: its heartbeat forgotten.)
            assert!(v.no_lead.as_deref().is_some_and(|w| w.contains("no heartbeat")), "{v:?}");
            assert!(v.alert.as_deref().is_some_and(|w| w.contains("the members finish their jobs and wait")), "{v:?}");
            assert!(said(&v, b.run.side.now())[0] == v.alert.clone().unwrap());
            drop(a);
        }
    }
}
