//! Tasks brokered by every member, pages talking to members directly (docs/pool.md §8, §9; §12
//! phase 4's tenth batch), behind the switch `state/pool/brokers` (`SWITCH`; off while missing or
//! `off` in it: the agent as it was, exactly). It acts only in the pool.
//!
//! - **Each member brokers its own jobs' tasks**: a member's agent runs a coordinator of its own
//!   (its broker, on the pool's port, crate::coord::PORT) and gives its jobs its address, so their
//!   tasks are offered there (crate::offload: `SCENIC_COORD`), served from its disk and handed back
//!   to it. The lead's coordinator brokers the lead's, as before.
//! - **A handover drops no task**: a part changing in its process (`state/pool/slots`), the lead's
//!   coordinator stays on as its broker once it no longer leads (crate::coord::Coordinator::
//!   become_broker: the jobs' leases left to the new lead), and a member's broker becomes its
//!   coordinator as it takes the lead (its tasks' leases kept as it takes up the term's: crate::
//!   coord's `load_pool_state`).
//! - **Its heartbeat says how many wait** (crate::agent::pool::Heartbeat's `broker`): its
//!   addresses, and its tasks waiting by kind, how many and the least memory one takes; given to its
//!   heartbeat at once (but not within `FLOOR` of the last) only when work comes (none to some);
//!   any other change, work going among them, at most every half minute (`due`): a heartbeat is a
//!   whole write on the NAS, and a task offered and taken again and again mustn't write each time.
//! - **Where is work**: each coordinator is told the members in touch and their brokers (`peers`,
//!   read from their heartbeats every half minute), and answers a page's `/work/where` with the
//!   brokers whose work fits it, most waiting first, the lead's address and every member's; and
//!   allows the members' own pages to call it (CORS: crate::coord::brokers).

use crate::agent::pool::Heartbeat;
use crate::coord::brokers::{Broker, Peer};
use std::path::Path;
use std::time::{Duration, Instant};

/// The switch (`on` in it, or present; `off` in it, or missing: off, the agent as it was).
pub const SWITCH: &str = "state/pool/brokers";

/// Whether tasks are brokered by every member, as the switch at `root` says; None when it can't
/// be read now.
pub fn on(root: &Path) -> Option<bool> {
    match std::fs::read_to_string(root.join(SWITCH)) {
        Ok(s) => Some(s.trim() != "off"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(false),
        Err(_) => None,
    }
}

/// How often the members' heartbeats are read for their brokers, and a broker's counts given to
/// its heartbeat at most (but for work coming or going).
pub const EVERY: Duration = Duration::from_secs(30);

/// The least time between two of a broker's heartbeat changes, work coming or not.
pub const FLOOR: Duration = Duration::from_secs(10);

/// How often a broker's address over HTTPS (`tailscale serve`, the owner's) is looked for again.
pub const HTTPS_EVERY: Duration = Duration::from_secs(300);

/// The members in touch and their brokers, from their heartbeats (`beats`: id, heartbeat; `lead`
/// the term's lead) at the reader's `now`: none out of touch, nor a shadow run's, nor one that
/// never beat. `own`: this Mac's member and broker, as it is now (its heartbeat on the NAS may lag).
pub fn peers(beats: &[(String, Heartbeat)], lead: Option<&str>, now: u64, own: (&str, &str, &Broker)) -> Vec<Peer> {
    let mut out: Vec<Peer> = beats
        .iter()
        .filter(|(id, h)| id != own.0 && !h.shadow && h.pool.beat > 0 && !h.pool.out_of_touch(now))
        .map(|(id, h)| Peer { member: id.clone(), host: if h.pool.host.is_empty() { id.clone() } else { h.pool.host.clone() }, lead: lead == Some(id.as_str()), broker: h.broker.clone() })
        .collect();
    out.push(Peer { member: own.0.to_string(), host: own.1.to_string(), lead: lead == Some(own.0), broker: Some(own.2.clone()) });
    out.sort_by(|a, b| a.host.cmp(&b.host).then(a.member.cmp(&b.member)));
    out
}

/// Whether a broker's heartbeat should say `now_b` in place of `was` (given `since` ago, None:
/// never): at once when work comes (none waited, some do), `FLOOR` after the last at least; any
/// other change once `EVERY` has passed.
pub fn due(was: Option<&Broker>, now_b: &Broker, since: Option<Duration>) -> bool {
    let Some(w) = was else { return true };
    if w == now_b {
        return false;
    }
    let some = |b: &Broker| b.waiting.values().any(|x| x.n > 0);
    let after = |d: Duration| since.is_none_or(|s| s >= d);
    (!some(w) && some(now_b) && after(FLOOR)) || after(EVERY)
}

/// When the last of something was done, and whether `every` has passed since (never done: yes).
pub fn stale(at: Option<Instant>, now: Instant, every: Duration) -> bool {
    at.is_none_or(|t| now.saturating_duration_since(t) >= every)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coord::brokers::Waiting;
    use crate::pool::beat::Beat;

    #[test]
    fn the_switch_is_off_while_missing_or_off() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(on(d.path()), Some(false));
        std::fs::create_dir_all(d.path().join("state/pool")).unwrap();
        std::fs::write(d.path().join(SWITCH), "off\n").unwrap();
        assert_eq!(on(d.path()), Some(false));
        std::fs::write(d.path().join(SWITCH), "").unwrap();
        assert_eq!(on(d.path()), Some(true));
    }

    fn b(urls: &[&str], waiting: u32) -> Broker {
        Broker { urls: urls.iter().map(|u| u.to_string()).collect(), https: None, waiting: if waiting > 0 { [("tail".to_string(), Waiting { n: waiting, mb: 500 })].into() } else { Default::default() }, out: 0 }
    }

    #[test]
    fn the_peers_are_the_members_in_touch_this_macs_own_as_it_is_now() {
        let hb = |id: &str, beat: u64, broker: Option<Broker>, shadow: bool| (id.to_string(), Heartbeat { pool: Beat { member: id.into(), host: format!("h{id}"), beat, ..Default::default() }, broker, shadow, ..Default::default() });
        let beats = vec![hb("a", 1000, Some(b(&["http://100.1.1.1:8090"], 2)), false), hb("b", 1000, None, false), hb("c", 300, Some(b(&[], 1)), false), hb("d", 1000, None, true), hb("e", 0, None, false), hb("me", 1000, Some(b(&["old"], 0)), false)];
        let own = b(&["http://100.1.1.9:8090"], 1);
        let p = peers(&beats, Some("a"), 1000, ("me", "hme", &own));
        let ids: Vec<&str> = p.iter().map(|p| p.member.as_str()).collect();
        assert_eq!(ids, ["a", "b", "me"], "out of touch, a shadow's and one that never beat left out");
        assert!(p[0].lead && !p[1].lead && !p[2].lead);
        assert_eq!(p[2].broker.as_ref(), Some(&own), "its own as it is, not as its heartbeat lags");
        assert_eq!(p[1].broker, None, "an app before the brokers");
    }

    #[test]
    fn a_brokers_heartbeat_changes_at_once_only_when_work_comes() {
        let s = Some(FLOOR);
        assert!(due(None, &b(&["u"], 0), None));
        assert!(!due(Some(&b(&["u"], 2)), &b(&["u"], 2), s));
        assert!(due(Some(&b(&["u"], 0)), &b(&["u"], 2), s), "work came");
        assert!(!due(Some(&b(&["u"], 0)), &b(&["u"], 2), Some(FLOOR - Duration::from_secs(1))), "not within the floor");
        assert!(!due(Some(&b(&["u"], 2)), &b(&["u"], 0), s), "work went: it waits");
        assert!(!due(Some(&b(&["u"], 2)), &b(&["v"], 2), s), "its address waits too");
        assert!(!due(Some(&b(&["u"], 2)), &b(&["u"], 3), s), "a count alone waits");
        assert!(due(Some(&b(&["u"], 2)), &b(&["u"], 0), Some(EVERY)));
        // A task offered and taken again and again, a second apart for ten minutes: work going is
        // written only half a minute on, and work coming ten seconds after that at least: two
        // writes each 40 s at most (not 600).
        let (mut told, mut at, mut writes) = (b(&["u"], 0), 0u64, 0);
        for t in 1..=600u64 {
            let now_b = b(&["u"], (t % 2) as u32);
            if due(Some(&told), &now_b, Some(Duration::from_secs(t - at))) {
                (told, at, writes) = (now_b, t, writes + 1);
            }
        }
        let most = 2 * 600 / (EVERY + FLOOR).as_secs() as usize + 2;
        assert!(writes <= most, "{writes} writes in ten minutes of flapping (at most {most})");
    }
}
