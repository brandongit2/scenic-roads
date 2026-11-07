//! The lead's answers to the owner's asks this Mac passed on (task #922): the build's pause
//! (`/work/pause`), a Mac's hold and the pool's settings (`/pool/hold`, `/pool/settings`,
//! crate::coord::asks), each last answer by what it's about (`key`), kept in the agent's folder
//! (`FILE`) so a restart keeps them.
//!
//! An ask the lead refuses as it is (a 400, crate::coord::client::refused; in process,
//! crate::coord::asks::Refused) leaves its queue, since asking again gets the same, and is said
//! here: what was asked, from which Mac, the lead's reason, when. The agent's status carries those
//! the owner hasn't seen (`unseen`, `Status::refused`), so the build page shows them by Mac and the
//! menu bar shows this Mac's, until the owner marks one seen (the page's Dismiss, `/work/asked`) or
//! a later ask for the same thing is taken (`note`). `scenic pause` and `resume` read the answer to
//! their own ask here (`of`).
//!
//! Only this Mac's agent process writes the file (its loop, its beat's poster, its coordinator's
//! `/work/asked`), one at a time (`locked`: a lock on `LOCK_FILE`, which any process honours), each
//! a whole document.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// The answers, in the agent's folder.
pub const FILE: &str = "asks-answered.json";

/// The build's pause, as a key (`/work/pause`).
pub const PAUSE: &str = "pause";

/// The lead's last answer to an ask about `key`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answered {
    /// What it's about: `pause` (the build's), `hold <member>`, `setting <name>`
    /// (crate::coord::asks::Ask::key).
    pub key: String,
    /// What was asked, in words.
    pub what: String,
    /// The Mac it was asked from (this one).
    pub mac: String,
    /// When it was asked (unix s, the ask's own `at`).
    pub at: u64,
    /// When the lead answered (unix s).
    pub answered: u64,
    /// The lead's reason, when it refused it; None: taken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    /// Whether the owner has seen the refusal (the page's Dismiss).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub seen: bool,
}

impl Answered {
    /// The answer to ask `key` (`what`, asked at `at` from this Mac): taken when `refused` is None.
    pub fn new(key: &str, what: &str, at: u64, refused: Option<String>) -> Answered {
        Answered {
            key: key.to_string(),
            what: what.to_string(),
            mac: store::sys::host().to_string(),
            at,
            answered: crate::agent::jobs::now_s(),
            refused,
            seen: false,
        }
    }
}

/// The lead's answer to queued ask `a` (crate::coord::asks) kept in `home`: taken, or refused
/// (`refused`); a member it holds named by its host as `beats` last heard it.
pub fn ask(home: &Path, beats: &crate::coord::beats::Beats, a: &crate::coord::asks::Ask, refused: Option<String>) {
    use crate::coord::asks::Ask;
    let name = match a {
        Ask::Hold(h) => beats
            .own()
            .filter(|o| o.pool.member == h.member)
            .or_else(|| beats.get(&h.member, crate::agent::jobs::now_s()))
            .map_or(h.member.clone(), |b| b.pool.host),
        Ask::Setting(_) => String::new(),
    };
    note(home, Answered::new(&a.key(), &a.what(&name), a.at(), refused));
}

/// The file the writers lock (`locked`), beside `FILE`: the answers themselves are replaced whole,
/// a new file each time, so a lock on them would hold nothing.
pub const LOCK_FILE: &str = "asks-answered.lock";

/// The answers' lock in `home`, held while it lives (none when it can't be had: written unlocked).
fn locked(home: &Path) -> Option<std::fs::File> {
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(home.join(LOCK_FILE)).ok()?;
    f.lock().ok()?;
    Some(f)
}

fn read(home: &Path) -> BTreeMap<String, Answered> {
    std::fs::read(home.join(FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write(home: &Path, all: &BTreeMap<String, Answered>) {
    let w = serde_json::to_vec_pretty(all)
        .map_err(anyhow::Error::from)
        .and_then(|b| store::whole::write(&home.join(FILE), &b));
    if let Err(e) = w {
        eprintln!("agent: the lead's answers to this Mac's asks: {e:#}");
    }
}

/// Answer `a` kept, unless one to a later ask about the same thing stands (an earlier ask's
/// answer, come late, never hides a later one's); logged when it's a refusal.
pub fn note(home: &Path, a: Answered) {
    if let Some(why) = &a.refused {
        eprintln!("agent: the lead refused this Mac's ask to {}: {why}", a.what);
    }
    let _g = locked(home);
    let mut all = read(home);
    if all.get(&a.key).is_some_and(|was| was.at > a.at) {
        return;
    }
    if all.get(&a.key) == Some(&a) {
        return;
    }
    all.insert(a.key.clone(), a);
    write(home, &all);
}

/// The refusals the owner hasn't seen, oldest ask first.
pub fn unseen(home: &Path) -> Vec<Answered> {
    let mut v: Vec<Answered> = read(home).into_values().filter(|a| a.refused.is_some() && !a.seen).collect();
    v.sort_by_key(|a| a.at);
    v
}

/// The refusal about `key` of the ask at `at` marked seen; whether there was one.
pub fn seen(home: &Path, key: &str, at: u64) -> bool {
    let _g = locked(home);
    let mut all = read(home);
    let Some(a) = all.get_mut(key).filter(|a| a.at == at && a.refused.is_some() && !a.seen) else {
        return false;
    };
    a.seen = true;
    write(home, &all);
    true
}

/// The lead's answer about `key` to the ask at `at`, or to a later one (which overtook it), once
/// there is one.
pub fn of(home: &Path, key: &str, at: u64) -> Option<Answered> {
    read(home).remove(key).filter(|a| a.at >= at)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A refusal stands until it's seen, or a later ask about the same thing is taken; an earlier
    /// ask's answer, come late, changes nothing.
    #[test]
    fn a_refusal_stands_until_seen_or_a_later_ask_is_taken() {
        let d = tempfile::tempdir().unwrap();
        let h = d.path();
        let no = |at| Answered::new("setting x", "set x", at, Some("not a setting the lead takes".into()));
        note(h, no(10));
        let u = unseen(h);
        assert_eq!((u.len(), u[0].at, u[0].mac.is_empty()), (1, 10, false));
        assert_eq!(of(h, "setting x", 10).and_then(|a| a.refused).as_deref(), Some("not a setting the lead takes"));
        assert!(of(h, "setting x", 11).is_none(), "not this ask's answer");
        // (Taken later: the refusal goes.)
        note(h, Answered::new("setting x", "set x", 12, None));
        assert!(unseen(h).is_empty());
        note(h, no(11));
        assert!(unseen(h).is_empty(), "an earlier ask's, come late");
        // (Seen: it goes, and only the one seen.)
        note(h, no(13));
        note(h, Answered::new(PAUSE, "pause the build", 5, Some("no".into())));
        assert!(!seen(h, "setting x", 12), "not the one refused");
        assert!(seen(h, "setting x", 13));
        assert_eq!(unseen(h).iter().map(|a| a.key.as_str()).collect::<Vec<_>>(), [PAUSE]);
        assert!(!seen(h, "setting x", 13), "once");
    }
}
