//! The pool (docs/pool.md): any Mac can lead the build. This is the core of its protocol, the part
//! that can lose work if it's wrong (§12, phases 1 and 2), kept apart from the agent and the
//! coordinator so the same code runs against the share and against a model of its faults (§13):
//!
//! - `nas`: the operations the protocol makes on the NAS, with what the SMB share guarantees and
//!   what it doesn't (§3);
//! - `term`: the terms, each lead's turn, a file made once with create-new (§6.1); the app rule;
//!   term 1, made by the Mac today's `state/build/writer` names; the owner's forced takeover;
//! - `records`: the build's records in one snapshot per term (§6.2), term 1's first made from
//!   today's three files; the merge of journal entries into them, and taking up a term;
//! - `journal`: the jobs' hand-offs, written by their members straight to the NAS as a log the lead
//!   merges (§7.3), and what a member keeps telling the lead until it's acknowledged;
//! - `handover`: handing the lead to another Mac, as pure transitions (§6.4);
//! - `beat`: the fields of a member's heartbeat the protocol reads and writes (§10);
//! - `driver`: what a member is in the pool and does as that, one step per loop of the agent's,
//!   through an I/O trait (the NAS's operations and the clocks): taking up terms, leading,
//!   re-asserting after a gap (§6.6), handing over and taking back, taking over, telling the lead
//!   of its entries; the API the agent's loop calls;
//! - `sim` (tests): the simulator, which runs the driver on two to four Macs on seeded schedules of
//!   sleeps, stale reads, delayed renames, busy files, cut creates and lost answers, and checks the
//!   invariants of §4 at every step and its progress at the end.
//!
//! Every file the protocol writes is one of three kinds (§2, principle 3): a member's own (its
//! heartbeat, its journal entries), one made once with create-new and never changed (terms, notes
//! of refusals), or one term's (its records); the lead's hint aside, which nothing takes for the
//! truth. So a Mac that slept through a change and writes late writes where no one reads:
//! nothing needs refusing (invariant 5).
//!
//! What the agent and the coordinator do around it (messages over HTTP, granting jobs, the
//! write-set checks they pass to the merge, the listings of the journal the driver asks for,
//! staying awake, the history) stays theirs; the simulator plays that part as the design has it
//! (`sim::Mac`).

pub mod beat;
pub mod driver;
pub mod handover;
pub mod journal;
pub mod nas;
pub mod records;
pub mod term;

#[cfg(test)]
mod sim;

pub use beat::Beat;
pub use driver::Driver;
pub use handover::Handover;
pub use journal::{Entry, LeaseId, Mine};
pub use nas::{Nas, Share};
pub use records::Records;
pub use term::{Current, Term};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;

/// A member as the pool names it (§5).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    /// Its id, made once (`member_id`), named in everything it writes.
    pub id: String,
    /// Its host name: only a label (renaming a Mac, or macOS adding "-2" after a clash, changes
    /// nothing).
    pub host: String,
    /// The app it runs (§6.1, the app rule).
    pub app: String,
}

/// The file in the agent's local folder that keeps this Mac's member id, and the Mac it's of.
const ID_FILE: &str = "member";

/// This Mac's member id (`m-<16 hex>`), kept in `home` (the agent's local folder) with the Mac's
/// hardware UUID (IOPlatformUUID), and made the first time, or again when the file names another
/// Mac: a copy Migration Assistant brought to a new Mac would make the two Macs one member. An
/// error when the file is there but can't be read now (a new id would make this Mac another
/// member).
pub fn member_id(home: &Path) -> Result<String> {
    member_id_on(home, platform_uuid())
}

/// `member_id` on the Mac whose hardware UUID is `mac` (None: it can't be read now).
fn member_id_on(home: &Path, mac: Option<&str>) -> Result<String> {
    let p = home.join(ID_FILE);
    match std::fs::read_to_string(&p) {
        Ok(s) => {
            let mut lines = s.lines().map(str::trim);
            let (id, of) = (lines.next().unwrap_or(""), lines.next().filter(|u| !u.is_empty()));
            match (of, mac) {
                // (Empty or cut short: its first write didn't finish, so nothing was named by it.)
                _ if !is_member_id(id) => {}
                // Another Mac's: this one is a member of its own.
                (Some(of), Some(mac)) if of != mac => {}
                // This Mac's; or this Mac's UUID can't be read now, and a new id would make it
                // another member.
                (Some(_), _) | (None, None) => return Ok(id.to_string()),
                // Made before its Mac could be read: bound now.
                (None, Some(mac)) => {
                    crate::whole::write(&p, format!("{id}\n{mac}\n").as_bytes())?;
                    return Ok(id.to_string());
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("read {}", p.display())),
    }
    std::fs::create_dir_all(home).with_context(|| format!("make {}", home.display()))?;
    let id = new_id();
    crate::whole::write(&p, format!("{id}\n{}\n", mac.unwrap_or("")).as_bytes())?;
    Ok(id)
}

/// This Mac's hardware UUID (IOPlatformUUID: the same across reinstalls, another on another Mac);
/// None when it can't be read.
fn platform_uuid() -> Option<&'static str> {
    static UUID: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    UUID.get_or_init(|| {
        let o = std::process::Command::new("/usr/sbin/ioreg").args(["-rd1", "-c", "IOPlatformExpertDevice"]).output().ok()?;
        parse_uuid(&String::from_utf8_lossy(&o.stdout))
    })
    .as_deref()
}

/// The IOPlatformUUID in `ioreg`'s output.
fn parse_uuid(ioreg: &str) -> Option<String> {
    ioreg.lines().find_map(|l| l.split_once("\"IOPlatformUUID\" = ")).map(|(_, v)| v.trim().trim_matches('"').to_string()).filter(|u| u.len() >= 32 && u.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'))
}

/// This process's hold on its member (crate::pool::driver's contract: one process per member): an
/// exclusive flock on a file named by the member id in this user's temporary folder, held while
/// the process runs, so a second process of the member (a second agent, or one started from a copy
/// of the agent's folder) can't take it. Let go when dropped.
pub struct MemberLock(#[allow(dead_code)] std::fs::File);

impl MemberLock {
    /// Member `id`'s lock; None when another process holds it.
    pub fn take(id: &str) -> Result<Option<MemberLock>> {
        anyhow::ensure!(is_member_id(id), "{id:?} isn't a member id");
        let p = std::env::temp_dir().join(format!("scenic-pool-{id}.lock"));
        let f = std::fs::File::options().create(true).truncate(false).write(true).open(&p).with_context(|| format!("open {}", p.display()))?;
        if !crate::sys::lock(&f, false).with_context(|| format!("lock {}", p.display()))? {
            return Ok(None);
        }
        Ok(Some(MemberLock(f)))
    }
}

/// Whether `s` is a member id: `m-` and 16 lowercase hex digits.
pub fn is_member_id(s: &str) -> bool {
    s.strip_prefix("m-").is_some_and(|h| h.len() == 16 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// A new member id: the system's random bytes, with the time, the process and the host name in
/// case they can't be read.
fn new_id() -> String {
    let mut seed = Vec::new();
    let mut b = [0u8; 16];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_ok() {
        seed.extend_from_slice(&b);
    }
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    seed.extend_from_slice(&t.to_le_bytes());
    seed.extend_from_slice(&std::process::id().to_le_bytes());
    seed.extend_from_slice(crate::agent::cond::host().as_bytes());
    format!("m-{}", store::naming::hash16(&seed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_member_id_is_made_once_and_kept() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("agent");
        let id = member_id(&home).unwrap();
        assert!(is_member_id(&id), "{id}");
        assert_eq!(member_id(&home).unwrap(), id);
        // Another Mac (another home) is another member.
        assert_ne!(member_id(&d.path().join("other")).unwrap(), id);
        // A file left empty by a first write cut short: made again.
        std::fs::write(home.join(ID_FILE), b"").unwrap();
        let again = member_id(&home).unwrap();
        assert!(is_member_id(&again) && again != id);
        // One that can't be read now is an error, not a new member.
        std::fs::remove_file(home.join(ID_FILE)).unwrap();
        std::fs::create_dir(home.join(ID_FILE)).unwrap();
        assert!(member_id(&home).is_err());
        assert!(!is_member_id("m-3F9C000000000000") && !is_member_id("m-3f9c") && !is_member_id("3f9c000000000000"));
    }

    #[test]
    fn a_member_is_one_process() {
        // (Review N7: nothing kept a second process of a member from stepping a driver.)
        let id = new_id();
        let held = MemberLock::take(&id).unwrap().expect("free");
        assert!(MemberLock::take(&id).unwrap().is_none(), "held by another");
        assert!(MemberLock::take(&new_id()).unwrap().is_some(), "another member's is its own");
        drop(held);
        assert!(MemberLock::take(&id).unwrap().is_some(), "let go");
        assert!(MemberLock::take("Mac-mini").is_err());
    }

    #[test]
    fn a_member_id_copied_to_another_mac_is_another_member() {
        // Migration Assistant copies the agent's folder to a new Mac. (Review L3: the id was a
        // plain file, and the two Macs were one member.)
        const OLD: &str = "6A1F0C2E-0000-4000-8000-00000000000A";
        const NEW: &str = "6A1F0C2E-0000-4000-8000-00000000000B";
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("agent");
        let id = member_id_on(&home, Some(OLD)).unwrap();
        assert_eq!(std::fs::read_to_string(home.join(ID_FILE)).unwrap(), format!("{id}\n{OLD}\n"));
        assert_eq!(member_id_on(&home, Some(OLD)).unwrap(), id);
        // Its UUID not read now: kept.
        assert_eq!(member_id_on(&home, None).unwrap(), id);
        // On the new Mac: a new member, bound to it.
        let copy = d.path().join("copy");
        std::fs::create_dir(&copy).unwrap();
        std::fs::copy(home.join(ID_FILE), copy.join(ID_FILE)).unwrap();
        let other = member_id_on(&copy, Some(NEW)).unwrap();
        assert!(is_member_id(&other) && other != id);
        assert_eq!(member_id_on(&copy, Some(NEW)).unwrap(), other);
        assert_eq!(member_id_on(&home, Some(OLD)).unwrap(), id, "the old Mac keeps its own");
        // One made while no UUID could be read: bound when one can.
        let early = d.path().join("early");
        let e = member_id_on(&early, None).unwrap();
        assert_eq!(member_id_on(&early, Some(NEW)).unwrap(), e);
        assert_eq!(std::fs::read_to_string(early.join(ID_FILE)).unwrap(), format!("{e}\n{NEW}\n"));
        assert_eq!(parse_uuid("    \"IOPlatformSerialNumber\" = \"X\"\n    \"IOPlatformUUID\" = \"6A1F0C2E-0000-4000-8000-00000000000A\"\n").as_deref(), Some(OLD));
        assert_eq!(parse_uuid("nothing"), None);
    }
}
