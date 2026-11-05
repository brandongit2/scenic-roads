//! Leases end only by their holder's word (docs/coordination.md §6, step 2 of task #878).
//!
//! - **The beat** (`POST /pool/beat`, `Beat`): each member's agent, every `agent::beat::EVERY`
//!   while it's a member (with its heartbeat and status: crate::coord::beats), lists the leases of the lead it holds (its slots' jobs, and those whose ends it still
//!   owes the lead). The lead confirms each listed and releases each of that member's jobs' leases
//!   left out (`lease::Leases::word`): a crashed job, an agent restarted without it, a grant whose
//!   answer never reached it (one granted within `OMIT_GRACE` excepted: its answer may still be on
//!   its way; one loaded at a restart, never). The answer says the lead's epoch and the leases listed
//!   that aren't that member's here (ended meanwhile: its jobs stop as their own beats are refused).
//! - **Gated by what the member speaks** (`SPEAKS`): the lead applies the rule to a member whose
//!   last beat says it speaks it; an older member's leases lapse after `coord::TTL` unrenewed as
//!   before, and an older lead answers the beat 404 (the member goes on as before). Each member's
//!   word is kept in the store (`kept::Heard`), so a lead's restart doesn't drop it.
//! - **A member not heard from** keeps its leases (they never lapse): held by an away member after
//!   `AWAY_S`, said in the status (`away`), and released after `AWAY_RELEASE_S` (12 h, the PM's
//!   decision of 30 Oct: a repeat costs doubled work only, the merge refusing the old lease's
//!   result) or by the owner (`scenic pool release <member>`, `POST /pool/release`).

use super::{history, kept, lease, Shared};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::time::Duration;

/// What a member's beat says it speaks: its leases end by its word.
pub const SPEAKS: &str = "held-leases";
/// A lease granted this recently isn't released by a beat leaving it out (its grant's answer may
/// be on its way still: a client's deadline is 5 s, its beat 10 s apart).
pub const OMIT_GRACE: Duration = Duration::from_secs(30);
/// A member unheard this long holds its leases as away (s).
pub const AWAY_S: u64 = 120;
/// And this long, its leases are released (s).
pub const AWAY_RELEASE_S: u64 = 12 * 3600;

/// A member's beat.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Beat {
    pub member: String,
    #[serde(default)]
    pub speaks: Vec<String>,
    /// The leases of the lead it holds.
    #[serde(default)]
    pub held: BTreeSet<u64>,
    /// How old its list `held` is (s): made by its agent's loop, posted by its beat's own thread
    /// while the loop may be stuck; no lease granted since is released for leaving it out. Left
    /// out by apps before it (their list as of the beat).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub held_age_s: u64,
    /// Its pool heartbeat (speaking `beats::SPEAKS`: crate::coord::beats).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heartbeat: Option<crate::agent::pool::Heartbeat>,
    /// Its agent's status (crate::agent::Status), its plan's parts left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<serde_json::Value>,
    /// The number up to which it took the lead's messages to it (speaking `beats::ASKS`): those
    /// the lead drops; the answer carries the rest.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub took: u64,
    /// Its jobs' hand-offs it keeps whole that the lead hasn't acknowledged, by key (speaking
    /// `beats::HANDOFF`, crate::coord::handoffs): the answer says which are merged and which the
    /// lead lacks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub handoffs: Vec<String>,
    /// Its Mac's capacity now (crate::coord::Capacity, as its asks carry it): with its running jobs'
    /// threads and use (`Capacity::threads`), whose threads the lead re-decides at each beat
    /// (docs/pool.md §7.1, Threads re-granted). Left out by apps before it; an older lead reads past it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cap: Option<super::Capacity>,
    /// Its resume progress whole (speaking `beats::BUDGETS`, crate::coord::budgets): the jobs whose
    /// progress is on its Mac's disk. Left out by apps before it, which write it on the NAS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<crate::agent::resume::Progress>,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// The lead's answer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    /// The lead's store's epoch (docs/coordination.md §4.3).
    pub epoch: u64,
    /// Leases it listed that aren't its here: ended (released, finished, or never granted).
    #[serde(default)]
    pub released: Vec<u64>,
    /// What the lead takes (`beats::SPEAKS`: the heartbeat and status in the beat); left out by a
    /// lead before it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speaks: Vec<String>,
    /// A member on an older app in touch still reads the NAS's heartbeats and statuses: write
    /// them there too.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub nas_too: bool,
    /// The other members' heartbeats as the lead hears them, its own among them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<crate::agent::pool::Heartbeat>,
    /// The lead's messages to this member after the number its beat said it took (`Beat::took`),
    /// each numbered (crate::pool::driver::Msg as JSON; docs/coordination.md step 5).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub msgs: Vec<(u64, serde_json::Value)>,
    /// This member's pause as the lead keeps it (`/pool/hold`: another Mac's, or its own).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<crate::control::HoldState>,
    /// The settings' generation (`/pool/settings`): a member fetches them when it moves.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub settings_gen: u64,
    /// A member heard doesn't say `beats::ASKS` (an older app): the NAS's mailboxes, holds and
    /// switch files go on beside (the rollout gate).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub asks_on_nas: bool,
    /// Of the hand-offs its beat listed (`Beat::handoffs`): those in a records snapshot the lead
    /// saved (the member drops them), and those the lead lacks (the member posts them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merged: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lacks: Vec<String>,
    /// The build in a line as the lead says it (docs/roles.md §3.4): what a member's menu bar shows
    /// of the build, kept by the member (`beats::LEAD_VIEW`). Left out by a lead before it, and
    /// until its agent first says its line (`Coordinator::set_said`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<LeadLine>,
    /// Its running jobs' threads re-decided (lease, threads: `sched::Sched::regrant`), each until
    /// its job runs on them: its agent tells each job, which takes them as its next target begins.
    /// Left out by a lead before it (the jobs keep their grant), and read past by a member before it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub threads: Vec<(u64, u32)>,
    /// A member heard doesn't say `beats::BUDGETS` (an older app): the Wikimedia budget and the
    /// answers' lease are taken on the NAS meanwhile, by every Mac (the rollout gate).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub budgets_on_nas: bool,
    /// The owner's releases of this member's progress the lead keeps (job, when its progress
    /// began: crate::coord::budgets).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub progress_released: Vec<(String, u64)>,
}

/// The build in a line, from the lead (`Answer::lead`): its Mac, its app, the build's state as its
/// agent says it (`agent::Status::build_said`), the build's pause, and whether every member is on
/// the new app (`Coordinator::all_new`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LeadLine {
    pub host: String,
    pub app: String,
    pub said: crate::agent::Verdict,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause: Option<crate::control::Pause>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub all_new: bool,
}

/// The members whose leases go by their word now: those speaking it, heard within
/// `AWAY_RELEASE_S`.
pub fn refresh(s: &mut Shared) {
    let unix = wall(s);
    s.leases.by_word = s.keep.speaking(SPEAKS, unix.saturating_sub(AWAY_RELEASE_S));
}

/// The coordinator's wall clock (unix s; a test's virtual one).
pub(super) fn wall(s: &Shared) -> u64 {
    s.clock.wall().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// `b` heard now: its member's word kept, its leases confirmed or released; the answer.
pub fn beat(s: &mut Shared, b: &Beat) -> Answer {
    let unix = wall(s);
    s.keep.heard(&b.member, b.speaks.clone(), unix);
    refresh(s);
    let epoch = s.keep.store().map_or(0, |st| st.epoch());
    // (Its heartbeat and status, in memory: crate::coord::beats. Without its agent's registry, as
    // before the pool's side is attached, the member's NAS copies go on.)
    let mut a = Answer { epoch, ..Default::default() };
    if let Some(beats) = s.beats.clone() {
        if let Some(h) = b.heartbeat.clone().filter(|_| b.speaks.iter().any(|w| w == super::beats::SPEAKS)) {
            let id = h.pool.member.clone();
            s.keep.beat(&id, &h, unix);
            beats.heard(h, b.speaks.clone(), b.status.clone(), unix);
            a.members = beats.members(unix, &id);
            if b.speaks.iter().any(|w| w == super::beats::ASKS) {
                if let Some(i) = s.inbox.as_ref() {
                    a.msgs = i.to(&id, b.took);
                }
                a.hold = s.asks.holds.get(&id).cloned();
            }
            if let Some(h) = s.handoffs.as_ref().filter(|_| b.speaks.iter().any(|w| w == super::beats::HANDOFF)) {
                (a.merged, a.lacks) = h.answer(&id, &b.handoffs);
            }
            if let Some(p) = b.progress.clone().filter(|p| p.member == id) {
                a.progress_released = super::budgets::heard(s, p);
            }
        }
        a.speaks = [super::beats::SPEAKS, super::beats::PAGE, super::beats::ASKS, super::beats::BUDGETS]
            .map(str::to_string)
            .to_vec();
        if s.handoffs.is_some() {
            a.speaks.push(super::beats::HANDOFF.to_string());
        }
        a.nas_too = beats.nas_too();
        a.lead = lead_line(s);
        a.settings_gen = s.asks.settings.gen;
        a.asks_on_nas = on_nas(s, &beats, unix, super::beats::ASKS);
        beats.set_asks_on_nas(a.asks_on_nas);
        a.budgets_on_nas = on_nas(s, &beats, unix, super::beats::BUDGETS);
    }
    if b.speaks.iter().any(|w| w == SPEAKS) {
        // (As of when its list was made: a lease granted since isn't in it.)
        let now = s.now();
        let now = now.checked_sub(Duration::from_secs(b.held_age_s)).unwrap_or(now);
        let (gone, released) = s.leases.word(&b.member, &b.held, now, OMIT_GRACE);
        ended(s, &gone, "its member's beat didn't list it: it doesn't hold it");
        a.released = released;
    }
    // (Its capacity, the one its asks carry, fresher; its running jobs' threads re-decided on it.)
    if let Some(cap) = b.cap.clone() {
        if let Some(w) = s.workers.get_mut(&b.member) {
            w.cap = Some(cap.clone());
        }
        a.threads = s.regrant(&b.member, cap);
    }
    a
}

/// The lead's line for its members' menu bars (`Answer::lead`): None until its agent says it.
fn lead_line(s: &Shared) -> Option<LeadLine> {
    let (host, said) = s.said.clone()?;
    let all_new = s.old_member(s.now()).is_none();
    Some(LeadLine { host, app: s.app.clone(), said, pause: s.paused.clone(), all_new })
}

/// Whether a member heard within `beats::FORGET_S` of `unix` doesn't say `word`, or one on an
/// older app heartbeats on the NAS alone (`Beats::nas_too`): that piece's NAS files go on (the
/// rollout gates of steps 5 and 7: `beats::ASKS`, `beats::BUDGETS`).
pub fn on_nas(s: &Shared, beats: &super::beats::Beats, unix: u64, word: &str) -> bool {
    let since = unix.saturating_sub(super::beats::FORGET_S);
    beats.nas_too() || s.keep.members().values().any(|h| h.at > since && !h.speaks.iter().any(|w| w == word))
}

/// The owner's release of `member`'s jobs' leases (`scenic pool release`: that Mac is gone for
/// good); those released.
pub fn release(s: &mut Shared, member: &str) -> Vec<lease::Lease> {
    let gone = s
        .leases
        .drop_where(|l| matches!(l.work, lease::Work::Job { .. }) && crate::agent::host_of(&l.worker) == member);
    ended(s, &gone, "released by the owner");
    gone
}

fn ended(s: &mut Shared, gone: &[lease::Lease], why: &str) {
    for l in gone {
        eprintln!("coordinator: {}'s lease {} ended: {why}", l.worker, l.id);
        s.history.add(history::Event {
            worker: Some(l.worker.clone()),
            lease: Some(l.id),
            step: match &l.work {
                lease::Work::Job { step, .. } => Some(step.clone()),
                lease::Work::Task { .. } => None,
            },
            targets: l.targets().iter().map(|t| t.0.clone()).collect(),
            note: why.to_string(),
            ..history::Event::new("lapse")
        });
    }
    if !gone.is_empty() {
        s.save_leases();
    }
}

/// The members whose leases are held while away (unheard `AWAY_S` or more, under
/// `AWAY_RELEASE_S`): each with its jobs' leases and when it was last heard (unix s).
pub fn away(s: &Shared) -> Vec<(String, usize, u64)> {
    let (now, unix) = (s.now(), wall(s));
    s.keep
        .members()
        .into_iter()
        .filter(|(_, h)| h.speaks.iter().any(|w| w == SPEAKS))
        .filter(|(_, h)| unix.saturating_sub(h.at) >= AWAY_S && unix.saturating_sub(h.at) < AWAY_RELEASE_S)
        .filter_map(|(m, h): (String, kept::Heard)| {
            let n = s
                .leases
                .live(now)
                .filter(|l| matches!(l.work, lease::Work::Job { .. }) && crate::agent::host_of(&l.worker) == m)
                .count();
            (n > 0).then_some((m, n, h.at))
        })
        .collect()
}
