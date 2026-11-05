//! Terms as a reader of the records finds them (docs/pool.md §6.1): a term is a file made once with
//! create-new and never changed, `state/build/terms/<E>.json`; the current term is the highest that
//! exists. A Mac learns it once (`current`), then checks only the next (`next`, a stat): nobody
//! writes a lower term, and no Mac's view goes back. Making terms and the declared lead are the
//! pool's (pipeline::pool::term, which re-exports these; task #916 step 6a, docs/crates.md §6).

use super::nas::Nas;
use super::Member;
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};

/// The folder of the terms (once it's there, no step saves the records, crate::out::check_writer).
pub const DIR: &str = crate::out::TERMS;
/// The lead's hint: the term it made or took up, for old apps and to save a listing.
pub const HINT: &str = "state/build/lead.json";

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
    /// How, in words: "the first Mac in the pool", "restarted: re-asserted", "re-asserted after a
    /// gap" (the elected leads' terms before #881 say "handed over by …", "taken over by …").
    pub how: String,
    /// The term before it (0 for the first).
    pub from: u64,
}

impl Term {
    /// The term after `cur`, led by `lead`, made `how` at `now` (unix seconds); not while the
    /// current term can't be read whole (its maker finishes it first).
    pub fn after(cur: &Current, lead: &Member, how: &str, now: u64) -> Result<Term> {
        ensure!(cur.term >= 1, "term 1 is made by `bootstrap`, its records first");
        ensure!(cur.lead.is_some(), "term {} can't be read whole yet", cur.term);
        Ok(Term {
            term: cur.term + 1,
            member: lead.id.clone(),
            host: lead.host.clone(),
            app: lead.app.clone(),
            since: now,
            how: how.to_string(),
            from: cur.term,
        })
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
    let hinted =
        nas.read(HINT)?.and_then(|b| serde_json::from_slice::<Term>(&b).ok()).map(|t| t.term).filter(|&e| e > 0);
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

/// Whether app `a` is at least as new as app `b` (the app rule). Published versions
/// (`20261005-2202-61eb22c`: the UTC minute it was published, then its commit) compare by that
/// minute; one that isn't published ("development") follows only another that isn't, and anything
/// follows it (a test lead never keeps the published app from leading).
/// A published app's version's moment (`YYYYMMDD-HHMM-…`), by bytes: a version is a member's to
/// say, and slicing it mid-character would panic. None: not a published app's ("development").
fn when(v: &str) -> Option<&[u8]> {
    let v = v.as_bytes();
    (v.len() > 14 && v[8] == b'-' && v[13] == b'-' && v[..8].iter().chain(&v[9..13]).all(u8::is_ascii_digit))
        .then(|| &v[..13])
}

/// Whether `v` is a published app's version (one `app_at_least` orders).
pub fn is_version(v: &str) -> bool {
    when(v).is_some()
}

pub fn app_at_least(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    match (when(a), when(b)) {
        (Some(x), Some(y)) => x >= y,
        (_, None) => true,
        (None, Some(_)) => false,
    }
}
