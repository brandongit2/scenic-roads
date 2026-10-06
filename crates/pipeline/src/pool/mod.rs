//! The pool (docs/pool.md): any Mac can lead the build. This is the core of its protocol, the part
//! that can lose work if it's wrong (§12, phases 1 and 2), kept apart from the agent and the
//! coordinator so the same code runs against the share and against a model of its faults (§13):
//!
//! - `nas`: the operations the protocol makes on the NAS, with what the SMB share guarantees and
//!   what it doesn't (§3);
//! - `term`: the terms, each lead's turn, a file made once with create-new (§6.1), the app rule,
//!   and term 1 made from today's `state/build/writer`;
//! - `records`: the build's records in one snapshot per term (§6.2), the merge of journal entries
//!   into them, and taking up a term;
//! - `journal`: the jobs' hand-offs, written by their members straight to the NAS as a log the lead
//!   merges (§7.3), and what a member keeps telling the lead until it's acknowledged;
//! - `handover`: handing the lead to another Mac, as pure transitions (§6.4), and the gaps after
//!   which a lead re-asserts (§6.6);
//! - `beat`: the fields of a member's heartbeat the protocol reads and writes (§10);
//! - `sim` (tests): the simulator, which runs two to four Macs on seeded schedules of sleeps, stale
//!   reads, delayed renames and busy files, and checks the invariants of §4 at every step.
//!
//! Every file the protocol writes is one of three kinds (§2, principle 3): a member's own (its
//! heartbeat, its journal entries), one made once with create-new and never changed (terms,
//! journal entries), or one term's (its records). So a Mac that slept through a change and writes
//! late writes where no one reads: nothing needs refusing (invariant 5).
//!
//! What the agent and the coordinator do with it (asks over HTTP, granting jobs, validating write
//! sets, staying awake, the history) is theirs: this module decides nothing they can get wrong
//! without it showing in its tests.

pub mod beat;
pub mod handover;
pub mod journal;
pub mod nas;
pub mod records;
pub mod term;

#[cfg(test)]
mod sim;

pub use beat::Beat;
pub use handover::Handover;
pub use journal::{Entry, LeaseId, Mine};
pub use nas::{Nas, Share};
pub use records::Records;
pub use term::{Current, Term};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;

/// A member as the pool names it (§5): its id, made once; its host name, only a label (renaming a
/// Mac, or macOS adding "-2" after a clash, changes nothing); the app it runs (§6.1, the app rule).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub id: String,
    pub host: String,
    pub app: String,
}

/// The file in the agent's local folder that keeps this Mac's member id.
const ID_FILE: &str = "member";

/// This Mac's member id (`m-<16 hex>`), kept in `home` (the agent's local folder) and made the
/// first time. An error when the file is there but can't be read now (a new id would make this Mac
/// another member).
pub fn member_id(home: &Path) -> Result<String> {
    let p = home.join(ID_FILE);
    match std::fs::read_to_string(&p) {
        Ok(s) if is_member_id(s.trim()) => return Ok(s.trim().to_string()),
        // (Empty or cut short: its first write didn't finish, so nothing was named by it.)
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("read {}", p.display())),
    }
    std::fs::create_dir_all(home).with_context(|| format!("make {}", home.display()))?;
    let id = new_id();
    crate::whole::write(&p, id.as_bytes())?;
    Ok(id)
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
}
