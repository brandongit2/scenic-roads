//! Terms (docs/pool.md §6.1): who leads the build, in turn. A term is a file made once with
//! create-new and never changed, `state/build/terms/<E>.json`. The current term is the highest that
//! exists, and its lead leads only while `<E+1>` doesn't: every change of lead (a handover, a
//! take-back, a takeover, a re-assertion) is the making of the next term, and of two Macs making it
//! at once one's create wins and the other stands down, with no lock (invariant 1). A Mac learns
//! the current term once (`current`), then checks only the next (`next`, a stat): nobody writes a
//! lower term, and no Mac's view goes back (invariant 2).

use super::nas::Nas;
use super::Member;
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};

/// The folder of the terms.
pub const DIR: &str = "state/build/terms";
/// The lead's hint: the term it made or took up, for old apps and to save a listing.
pub const HINT: &str = "state/build/lead.json";
/// Today's naming of the build Mac (by host name): term 1's lead (§12, phase 1).
pub const WRITER: &str = "state/build/writer";
/// What a forced term's `how` says when the term before it couldn't be read whole (`force`).
pub const UNREAD: &str = "forced past";

/// A term's file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Term {
    /// Its number, from 1.
    pub term: u64,
    /// Its lead: member id, host name (a label), and app (the app rule).
    pub member: String,
    pub host: String,
    pub app: String,
    /// When it was made (unix seconds, by its maker's clock).
    pub since: u64,
    /// How, in words: "handed over by …", "taken back: … didn't take up", "taken over by …",
    /// "re-asserted after a gap".
    pub how: String,
    /// The term before it (0 for the first).
    pub from: u64,
    /// For a handover: the sequence number of term `from`'s last records snapshot, which the new
    /// lead must read before taking up (a stale read of an older one would lack what the old lead
    /// merged last).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
}

impl Term {
    /// The term after `cur`, led by `lead`, made `how` at `now` (unix seconds): refused by the app
    /// rule when `lead`'s app is older than the current term's (job keys include the steps'
    /// versions: a lead on an older app would take what a newer one built as stale), and while the
    /// current term can't be read whole (its app isn't known: wait, or `force`).
    pub fn after(cur: &Current, lead: &Member, how: &str, now: u64) -> Result<Term> {
        ensure!(cur.term >= 1, "term 1 is made by `bootstrap`, its records first");
        let Some(c) = &cur.lead else { bail!("term {} can't be read whole yet: its lead and app aren't known", cur.term) };
        ensure!(app_at_least(&lead.app, &c.app), "{} runs app {}, older than term {}'s {}: update it first", lead.host, lead.app, c.term, c.app);
        Ok(Term { term: cur.term + 1, member: lead.id.clone(), host: lead.host.clone(), app: lead.app.clone(), since: now, how: how.to_string(), from: cur.term, seq: None })
    }

    /// Its lead.
    pub fn lead(&self) -> Member {
        Member { id: self.member.clone(), host: self.host.clone(), app: self.app.clone() }
    }
}

/// The current term as a Mac knows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Current {
    /// The highest term found; 0 before the first is made.
    pub term: u64,
    /// That term's file; None while it can't be read whole (being made, or its maker stopped
    /// before its bytes landed: then no one leads until a later term).
    pub lead: Option<Term>,
}

/// The path of term `e`'s file.
pub fn path(e: u64) -> String {
    format!("{DIR}/{e}.json")
}

/// Term `e`'s file; None when it isn't there or can't be read whole yet (`next` or the NAS's
/// `exists` tell which, where it matters). An error when it can't be read now, or names another
/// term.
pub fn read(nas: &dyn Nas, e: u64) -> Result<Option<Term>> {
    let Some(b) = nas.read(&path(e))? else { return Ok(None) };
    // (Made with create-new, its bytes after: empty or short until they land.)
    let Ok(t) = serde_json::from_slice::<Term>(&b) else { return Ok(None) };
    ensure!(t.term == e, "{} is term {}'s", path(e), t.term);
    Ok(Some(t))
}

/// Whether term `e + 1` exists: what a lead of term `e` checks every loop before acting, and a
/// member to learn of a new lead (a stat, never a listing).
pub fn next(nas: &dyn Nas, e: u64) -> Result<bool> {
    nas.exists(&path(e + 1))
}

/// The current term: from the lead's hint, or a listing of `terms/` when there's none, then
/// checked upward from there (either may be stale, never ahead). Once, at a Mac's start.
pub fn current(nas: &dyn Nas) -> Result<Current> {
    let hinted = nas.read(HINT)?.and_then(|b| serde_json::from_slice::<Term>(&b).ok()).map(|t| t.term).filter(|&e| e > 0);
    let mut e = match hinted {
        Some(e) if nas.exists(&path(e))? => e,
        _ => nas.list(DIR)?.iter().filter_map(|n| n.strip_suffix(".json")?.parse::<u64>().ok()).max().unwrap_or(0),
    };
    while next(nas, e)? {
        e += 1;
    }
    let lead = if e > 0 { read(nas, e)? } else { None };
    Ok(Current { term: e, lead })
}

/// Makes term `t` with create-new: true when this Mac's create made it (or an earlier try of the
/// same file did), false when another Mac's did. Term 1 only once its records are made
/// (crate::pool::records::first). (An earlier try that made the file and failed before its bytes
/// landed can't be told from another Mac's still being written: it reads as another's, and the
/// term has no lead until the owner's `force`.)
pub fn make(nas: &dyn Nas, t: &Term) -> Result<bool> {
    ensure!(t.term >= 1 && t.from + 1 == t.term, "term {} can't follow term {}", t.term, t.from);
    ensure!(t.term > 1 || nas.exists(&super::records::path(1))?, "term 1's records aren't made yet");
    let b = serde_json::to_vec_pretty(t)?;
    if nas.create_new(&path(t.term), &b).with_context(|| format!("make term {}", t.term))? {
        return Ok(true);
    }
    Ok(nas.read(&path(t.term))?.is_some_and(|got| got == b))
}

/// Makes the term after `cur` naming `me` (a takeover, a re-assertion, a take-back): the term when
/// this Mac's create won, None when another's did. Refused by the app rule.
pub fn claim(nas: &dyn Nas, cur: &Current, me: &Member, how: &str, now: u64) -> Result<Option<Term>> {
    let t = Term::after(cur, me, how, now)?;
    Ok(make(nas, &t)?.then_some(t))
}

/// The owner's forced takeover (§6.5, `scenic lead take --force`): `claim`, and also past a current
/// term that can't be read whole (its maker stopped between its create and its bytes, and may never
/// write them: no one leads it), the app rule then checked against the newest term that can be read,
/// and the term's `how` saying so.
pub fn force(nas: &dyn Nas, cur: &Current, me: &Member, how: &str, now: u64) -> Result<Option<Term>> {
    if cur.lead.is_some() || cur.term == 0 {
        return claim(nas, cur, me, how, now);
    }
    let mut known = None;
    for e in (1..cur.term).rev() {
        if let Some(t) = read(nas, e)? {
            known = Some(t);
            break;
        }
    }
    if let Some(k) = &known {
        ensure!(app_at_least(&me.app, &k.app), "{} runs app {}, older than term {}'s {}: update it first", me.host, me.app, k.term, k.app);
    }
    let t = Term { term: cur.term + 1, member: me.id.clone(), host: me.host.clone(), app: me.app.clone(), since: now, how: format!("{how} ({}: term {} unreadable)", UNREAD, cur.term), from: cur.term, seq: None };
    Ok(make(nas, &t)?.then_some(t))
}

/// Term 1 (§12, phase 1), once the pool is switched on: made by the Mac `state/build/writer` names
/// (today's build Mac, by host name), naming itself, or by any Mac when none is named or the owner
/// has this one take the lead (`force`); its records first, from today's files
/// (crate::pool::records::first). The term once it's made (by this Mac or another); None while
/// it's still the writer's to make, or being made.
pub fn bootstrap(nas: &dyn Nas, me: &Member, now: u64, force: bool) -> Result<Option<Term>> {
    if nas.exists(&path(1))? {
        return read(nas, 1);
    }
    let writer = nas.read(WRITER)?.map(|b| String::from_utf8_lossy(&b).trim().to_string()).filter(|w| !w.is_empty());
    let how = match &writer {
        _ if force => format!("taken over by {}", me.host),
        Some(w) if *w != me.host => return Ok(None),
        Some(_) => "the build Mac when the pool began".to_string(),
        None => "the first Mac in the pool".to_string(),
    };
    super::records::first(nas)?;
    let t = Term { term: 1, member: me.id.clone(), host: me.host.clone(), app: me.app.clone(), since: now, how, from: 0, seq: None };
    if make(nas, &t)? {
        Ok(Some(t))
    } else {
        read(nas, 1)
    }
}

/// Writes the lead's hint, `state/build/lead.json` (§6.1): never the truth (a stale lead may write
/// it late; `current` checks upward from it).
pub fn write_hint(nas: &dyn Nas, t: &Term) -> Result<()> {
    nas.write_whole(HINT, &serde_json::to_vec_pretty(t)?)
}

/// Whether app `a` is at least as new as app `b` (the app rule). Published versions
/// (`20261005-2202-61eb22c`: the UTC minute it was published, then its commit) compare by that
/// minute; one that isn't published ("development") follows only another that isn't, and anything
/// follows it (a test lead never keeps the published app from leading).
pub fn app_at_least(a: &str, b: &str) -> bool {
    // (By bytes: a version is a member's to say, and slicing it mid-character would panic.)
    fn when(v: &str) -> Option<&[u8]> {
        let v = v.as_bytes();
        (v.len() > 14 && v[8] == b'-' && v[13] == b'-' && v[..8].iter().chain(&v[9..13]).all(u8::is_ascii_digit)).then(|| &v[..13])
    }
    if a == b {
        return true;
    }
    match (when(a), when(b)) {
        (Some(x), Some(y)) => x >= y,
        (_, None) => true,
        (None, Some(_)) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::nas::Mem;
    use super::*;

    fn member(id: &str, app: &str) -> Member {
        Member { id: id.into(), host: format!("{id}-host"), app: app.into() }
    }

    #[test]
    fn the_current_term_is_the_highest_found_by_one_listing_then_checks_upward() {
        let nas = Mem::default();
        assert_eq!(current(&nas).unwrap(), Current::default());
        let a = member("m-000000000000000a", "20261005-2202-61eb22c");
        let t1 = bootstrap(&nas, &a, 100, false).unwrap().unwrap();
        let t2 = claim(&nas, &current(&nas).unwrap(), &a, "re-asserted after a gap", 200).unwrap().unwrap();
        assert_eq!(current(&nas).unwrap(), Current { term: 2, lead: Some(t2.clone()) });
        // A hint behind the truth is checked upward from; one naming no term is passed over.
        write_hint(&nas, &t1).unwrap();
        assert_eq!(current(&nas).unwrap().term, 2);
        nas.write_whole(HINT, &serde_json::to_vec(&Term { term: 9, ..t1.clone() }).unwrap()).unwrap();
        assert_eq!(current(&nas).unwrap().term, 2);
        // A term made, its bytes not landed yet: current, its lead unknown.
        nas.create_new(&path(3), b"").unwrap();
        assert_eq!(current(&nas).unwrap(), Current { term: 3, lead: None });
        assert!(next(&nas, 2).unwrap() && !next(&nas, 3).unwrap());
    }

    #[test]
    fn of_two_macs_making_a_term_one_wins_and_a_retry_knows_its_own() {
        let nas = Mem::default();
        let (a, b) = (member("m-000000000000000a", "20261005-2202-61eb22c"), member("m-000000000000000b", "20261005-2202-61eb22c"));
        // Term 1 only by bootstrap, its records first.
        assert!(Term::after(&Current::default(), &a, "taken over", 100).is_err());
        let t1 = Term { term: 1, member: a.id.clone(), host: a.host.clone(), app: a.app.clone(), since: 100, how: "by hand".into(), from: 0, seq: None };
        assert!(make(&nas, &t1).is_err() && !nas.exists(&path(1)).unwrap());
        let t1 = bootstrap(&nas, &a, 100, false).unwrap().unwrap();
        let cur = current(&nas).unwrap();
        let ta = Term::after(&cur, &a, "taken over", 200).unwrap();
        let tb = Term::after(&cur, &b, "taken over", 200).unwrap();
        assert!(make(&nas, &ta).unwrap());
        assert!(!make(&nas, &tb).unwrap(), "b's create lost");
        assert!(make(&nas, &ta).unwrap(), "a's own, tried again (its first call's answer lost)");
        assert_eq!(read(&nas, 2).unwrap(), Some(ta.clone()));
        assert_eq!(read(&nas, 1).unwrap(), Some(t1));
        // A term can only follow the one before it.
        assert!(make(&nas, &Term { term: 4, ..ta.clone() }).is_err());
        // A file under another term's name is an error.
        nas.create_new(&path(3), &serde_json::to_vec(&ta).unwrap()).unwrap();
        assert!(read(&nas, 3).is_err());
    }

    #[test]
    fn the_app_rule_refuses_an_older_app() {
        let a = member("m-000000000000000a", "20261005-2202-61eb22c");
        let cur = Current { term: 4, lead: Some(Term { term: 4, member: a.id, host: a.host, app: a.app, since: 0, how: "handed over".into(), from: 3, seq: None }) };
        assert!(Term::after(&cur, &member("m-000000000000000b", "20261004-0910-1a2b3c4"), "handed over", 1).is_err());
        let t = Term::after(&cur, &member("m-000000000000000b", "20261012-0910-1a2b3c4"), "handed over", 1).unwrap();
        assert_eq!((t.term, t.from, t.member.as_str()), (5, 4, "m-000000000000000b"));
        // Its lead unknown (its file being made): not until it's known, or by force.
        assert!(Term::after(&Current { term: 4, lead: None }, &member("m-000000000000000b", "development"), "taken over", 1).is_err());
        assert!(app_at_least("20261005-2202-61eb22c", "20261005-2202-0000000"), "the same minute");
        assert!(app_at_least("20261005-2203-61eb22c", "20261005-2202-61eb22c"));
        assert!(!app_at_least("20261005-2201-61eb22c", "20261005-2202-61eb22c"));
        assert!(app_at_least("development", "development") && app_at_least("20261005-2202-61eb22c", "development"));
        assert!(!app_at_least("development", "20261005-2202-61eb22c") && !app_at_least("", "20261005-2202-61eb22c"));
        assert!(!app_at_least("2026100é-2202-61eb22c", "20261005-2202-61eb22c"), "not a version");
    }

    #[test]
    fn a_forced_takeover_passes_a_term_that_cant_be_read_but_not_the_app_rule() {
        let nas = Mem::default();
        let (a, b) = (member("m-000000000000000a", "20261005-2202-61eb22c"), member("m-000000000000000b", "20261005-2202-61eb22c"));
        bootstrap(&nas, &a, 100, false).unwrap().unwrap();
        // Term 2 made, its maker stopped before its bytes: no one leads it.
        nas.create_new(&path(2), b"").unwrap();
        let cur = current(&nas).unwrap();
        assert_eq!(cur, Current { term: 2, lead: None });
        assert!(claim(&nas, &cur, &b, "taken over by MacBook-Air", 200).is_err(), "unforced, it waits");
        let old = member("m-000000000000000b", "20261004-0000-61eb22c");
        assert!(force(&nas, &cur, &old, "taken over by MacBook-Air", 200).is_err(), "older than term 1's app");
        let t = force(&nas, &cur, &b, "taken over by MacBook-Air", 200).unwrap().unwrap();
        assert_eq!((t.term, t.from, t.how.as_str()), (3, 2, "taken over by MacBook-Air (forced past: term 2 unreadable)"));
        // A term that can be read: as `claim`.
        let t4 = force(&nas, &current(&nas).unwrap(), &a, "taken over by Mac-mini", 300).unwrap().unwrap();
        assert_eq!((t4.term, t4.how.as_str()), (4, "taken over by Mac-mini"));
    }

    #[test]
    fn term_one_is_the_writers_to_make() {
        let nas = Mem::default();
        let (m4, m1) = (Member { id: "m-0000000000000004".into(), host: "Mac-mini".into(), app: "development".into() }, Member { id: "m-0000000000000001".into(), host: "MacBook-Air".into(), app: "development".into() });
        nas.write_whole(WRITER, b"Mac-mini\n").unwrap();
        nas.write_whole("state/build/manifest.json", br#"{"base/6-1-1": "base/6-1-1.k0.base"}"#).unwrap();
        assert_eq!(bootstrap(&nas, &m1, 10, false).unwrap(), None, "the M4's to make");
        assert!(!nas.exists(&super::super::records::path(1)).unwrap());
        let t = bootstrap(&nas, &m4, 11, false).unwrap().unwrap();
        assert_eq!((t.term, t.member.as_str(), t.from), (1, "m-0000000000000004", 0));
        assert_eq!(bootstrap(&nas, &m1, 12, false).unwrap(), Some(t), "then every Mac sees it");
        // Its records made first, from today's files.
        let r = super::super::Records::load(&nas, 1).unwrap().unwrap();
        assert_eq!((r.term, r.seq, r.manifest.len()), (1, 1, 1));
        // No writer named, the first Mac to start makes it; the owner's say-so, any Mac.
        let fresh = Mem::default();
        assert_eq!(bootstrap(&fresh, &m1, 13, false).unwrap().map(|t| t.member), Some(m1.id.clone()));
        let named = Mem::default();
        named.write_whole(WRITER, b"Mac-mini\n").unwrap();
        assert_eq!(bootstrap(&named, &m1, 14, true).unwrap().map(|t| (t.member, t.how)), Some((m1.id.clone(), "taken over by MacBook-Air".to_string())));
    }
}
