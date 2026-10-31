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
    /// Its pool heartbeat (speaking `beats::SPEAKS`: crate::coord::beats).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heartbeat: Option<crate::agent::pool::Heartbeat>,
    /// Its agent's status (crate::agent::Status), its plan's parts left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<serde_json::Value>,
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
}

/// The members whose leases go by their word now: those speaking it, heard within
/// `AWAY_RELEASE_S`.
pub fn refresh(s: &mut Shared) {
    let unix = wall(s);
    s.leases.by_word = s.keep.speaking(SPEAKS, unix.saturating_sub(AWAY_RELEASE_S));
}

/// The coordinator's wall clock (unix s; a test's virtual one).
fn wall(s: &Shared) -> u64 {
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
            beats.heard(h, b.status.clone(), unix);
            a.members = beats.members(unix, &id);
        }
        a.speaks = vec![super::beats::SPEAKS.to_string()];
        a.nas_too = beats.nas_too();
    }
    if b.speaks.iter().any(|w| w == SPEAKS) {
        let now = s.now();
        let (gone, released) = s.leases.word(&b.member, &b.held, now, OMIT_GRACE);
        ended(s, &gone, "its member's beat didn't list it: it doesn't hold it");
        a.released = released;
    }
    a
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
