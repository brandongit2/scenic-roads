//! The pool's lead as a reader of the records finds it (docs/pool.md §6.1): declared once, in
//! `state/build/lead-member.json` (`declared`), and fixed (task #881): the Mac mini, moved only by
//! the owner's edit. A Mac's role is by that declaration (`role`; step 7a: the map server's retire
//! check reads it too); the declaration's being there is the pool's having begun (super::now: no
//! reader takes today's files for the records then; crate::out::check_writer: no step saves them).
//! The app rule (`app_at_least`) orders the apps the members run.

use super::nas::{Created, Nas};
use super::Member;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// The declaration of the lead (`Declared`): which member leads, for good.
pub const LEAD: &str = "state/build/lead-member.json";

/// The declared lead: the one member that leads, its store's epoch the records' and the leases'
/// number (docs/coordination.md §4.3). Written once with create-new (`declare`) by the first Mac in
/// a new pool; moved to another Mac by the owner, by hand (docs/pool.md §6.1). (A `since` an older
/// app wrote is read past.)
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Declared {
    /// Its member id.
    pub member: String,
    /// Its host name: a label.
    #[serde(default)]
    pub host: String,
}

/// The declared lead; None when there's no declaration yet (or it can't be read whole: being made).
pub fn declared(nas: &dyn Nas) -> Result<Option<Declared>> {
    Ok(nas.read(LEAD)?.and_then(|b| serde_json::from_slice::<Declared>(&b).ok()).filter(|d| !d.member.is_empty()))
}

/// This Mac's role (docs/roles.md §1), decided once as its agent starts and by the server's retire
/// check: the declared lead's member (`declared`) against this Mac's own (`me`, its member file).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// The declared lead: it plans, coordinates and serves.
    Lead,
    /// Another member is declared: this Mac works only.
    Member,
    /// Not known: the declaration or this Mac's member id unread (no NAS, no member file yet, no
    /// declaration). It runs a member's parts (a subset of the lead's) until it is known.
    Unknown,
}

/// This Mac's role: `me` its member id (None: no member file yet).
pub fn role(nas: &dyn Nas, me: Option<&str>) -> Role {
    role_of(declared(nas).ok().flatten().map(|d| d.member).as_deref(), me)
}

/// The role by the declared lead's member id (None: unread or none declared) and this Mac's.
pub fn role_of(lead: Option<&str>, me: Option<&str>) -> Role {
    match (lead, me) {
        (Some(l), Some(m)) if l == m => Role::Lead,
        (Some(_), Some(_)) => Role::Member,
        _ => Role::Unknown,
    }
}

/// Declares `me` the lead when no lead is declared yet (create-new: one declaration, but for the
/// owner's hand): the declaration there now, this one or an earlier.
pub fn declare(nas: &dyn Nas, me: &Member) -> Result<Option<Declared>> {
    let d = serde_json::to_vec_pretty(&Declared { member: me.id.clone(), host: me.host.clone() })?;
    if let Created::Unwritten(e) = nas.create_new(LEAD, &d)? {
        // (Its create made the file and its bytes didn't land: no other create can win it.)
        nas.write_whole(LEAD, &d).with_context(|| format!("{e:#}"))?;
    }
    declared(nas)
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
