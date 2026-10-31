//! Tasks brokered by every member, members' pages talking to members directly (docs/pool.md §8, §9; §12
//! phase 4's tenth batch), behind the switch `state/pool/brokers` (`SWITCH`; off while missing or
//! `off` in it: the agent as it was, exactly). It acts only in the pool.
//!
//! - **Each member brokers its own jobs' tasks**: a member's agent runs a coordinator of its own
//!   (its broker, on the pool's port, crate::coord::PORT) and gives its jobs its address, so their
//!   tasks are offered there (crate::offload: `SCENIC_COORD`), served from its disk and handed back
//!   to it. The lead's coordinator brokers the lead's, as before.
//! - **A change of lead** (the owner's move of the declaration) restarts both Macs' agents into
//!   their new roles (docs/roles.md §1), their jobs handed over to the next process; a job taken
//!   over offers its tasks through the new process's coordinator or broker on the same port (the
//!   jobs' token kept: docs/building.md, Restarts).
//! - **Its heartbeat says where it is** (crate::agent::pool::Heartbeat's `broker`): its addresses,
//!   given to its heartbeat when they change, at most every half minute (`due`): a heartbeat is a
//!   whole write on the NAS.
//! - **Where the members are**: each coordinator is told the members in touch and their brokers
//!   (`peers`, read from their heartbeats every half minute, in the pool's run's steps, the loop's
//!   or a tick's, under the run's lock: crate::agent::pool::Run::tend_brokers), and allows the
//!   members' own pages to call it (CORS: crate::coord::brokers).

use crate::agent::pool::Heartbeat;
use crate::coord::brokers::{Broker, Peer};
use std::time::{Duration, Instant};

/// The switch (`on` in it, or present; `off` in it, or missing: off, the agent as it was).
pub const SWITCH: &str = "state/pool/brokers";

/// Whether tasks are brokered by every member, as the switch at `root` says; None when it can't
/// be read now.
pub fn on(root: &store::nasfs::NasRoot) -> Option<bool> {
    match super::nasread::read_switch(&root.join(SWITCH)) {
        Ok(s) => Some(s.trim() != "off"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(false),
        Err(_) => None,
    }
}

/// How often the members' heartbeats are read for their brokers, and a broker's addresses given
/// to its heartbeat at most.
pub const EVERY: Duration = Duration::from_secs(30);

/// How often a broker's address over HTTPS (`tailscale serve`, the owner's) is looked for again.
pub const HTTPS_EVERY: Duration = Duration::from_secs(300);

/// The members in touch and their brokers, from their heartbeats (`beats`: id, heartbeat; `lead`
/// the term's lead) at the reader's `now`: none out of touch, nor a shadow run's, nor one that
/// never beat. `own`: this Mac's member and broker, as it is now (its heartbeat on the NAS may lag).
pub fn peers(beats: &[(String, Heartbeat)], lead: Option<&str>, now: u64, own: (&str, &str, &Broker)) -> Vec<Peer> {
    let mut out: Vec<Peer> = beats
        .iter()
        .filter(|(id, h)| id != own.0 && !h.shadow && h.pool.beat > 0 && !h.pool.out_of_touch(now))
        .map(|(id, h)| Peer {
            member: id.clone(),
            host: if h.pool.host.is_empty() { id.clone() } else { h.pool.host.clone() },
            lead: lead == Some(id.as_str()),
            broker: h.broker.clone(),
        })
        .collect();
    out.push(Peer {
        member: own.0.to_string(),
        host: own.1.to_string(),
        lead: lead == Some(own.0),
        broker: Some(own.2.clone()),
    });
    out.sort_by(|a, b| a.host.cmp(&b.host).then(a.member.cmp(&b.member)));
    out
}

/// Whether a broker's heartbeat should say `now_b` in place of `was` (given `since` ago, None:
/// never): at once when it said none, a change once `EVERY` has passed.
pub fn due(was: Option<&Broker>, now_b: &Broker, since: Option<Duration>) -> bool {
    let Some(w) = was else { return true };
    w != now_b && since.is_none_or(|s| s >= EVERY)
}

/// When the last of something was done, and whether `every` has passed since (never done: yes).
pub fn stale(at: Option<Instant>, now: Instant, every: Duration) -> bool {
    at.is_none_or(|t| now.saturating_duration_since(t) >= every)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::beat::Beat;

    #[test]
    fn the_switch_is_off_while_missing_or_off() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(on(&store::nasfs::NasRoot::local_for_test(d.path())), Some(false));
        std::fs::create_dir_all(d.path().join("state/pool")).unwrap();
        std::fs::write(d.path().join(SWITCH), "off\n").unwrap();
        assert_eq!(on(&store::nasfs::NasRoot::local_for_test(d.path())), Some(false));
        std::fs::write(d.path().join(SWITCH), "").unwrap();
        assert_eq!(on(&store::nasfs::NasRoot::local_for_test(d.path())), Some(true));
    }

    fn b(urls: &[&str]) -> Broker {
        Broker { urls: urls.iter().map(|u| u.to_string()).collect(), ..Default::default() }
    }

    #[test]
    fn the_peers_are_the_members_in_touch_this_macs_own_as_it_is_now() {
        let hb = |id: &str, beat: u64, broker: Option<Broker>, shadow: bool| {
            (
                id.to_string(),
                Heartbeat {
                    pool: Beat { member: id.into(), host: format!("h{id}"), beat, ..Default::default() },
                    broker,
                    shadow,
                    ..Default::default()
                },
            )
        };
        let beats = vec![
            hb("a", 1000, Some(b(&["http://100.1.1.1:8090"])), false),
            hb("b", 1000, None, false),
            hb("c", 300, Some(b(&[])), false),
            hb("d", 1000, None, true),
            hb("e", 0, None, false),
            hb("me", 1000, Some(b(&["old"])), false),
        ];
        let own = b(&["http://100.1.1.9:8090"]);
        let p = peers(&beats, Some("a"), 1000, ("me", "hme", &own));
        let ids: Vec<&str> = p.iter().map(|p| p.member.as_str()).collect();
        assert_eq!(ids, ["a", "b", "me"], "out of touch, a shadow's and one that never beat left out");
        assert!(p[0].lead && !p[1].lead && !p[2].lead);
        assert_eq!(p[2].broker.as_ref(), Some(&own), "its own as it is, not as its heartbeat lags");
        assert_eq!(p[1].broker, None, "an app before the brokers");
    }

    #[test]
    fn a_brokers_heartbeat_changes_only_when_its_addresses_do_every_half_minute_at_most() {
        assert!(due(None, &b(&["u"]), None));
        assert!(!due(Some(&b(&["u"])), &b(&["u"]), Some(EVERY)));
        assert!(!due(Some(&b(&["u"])), &b(&["v"]), Some(EVERY - Duration::from_secs(1))), "within half a minute");
        assert!(due(Some(&b(&["u"])), &b(&["v"]), Some(EVERY)));
        assert!(due(Some(&b(&["u"])), &b(&["v"]), None));
    }
}
