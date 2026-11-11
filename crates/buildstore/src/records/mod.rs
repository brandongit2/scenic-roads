//! The build's records as a reader finds them (docs/pool.md §6.2; task #916 step 6a,
//! docs/crates.md §6): the NAS's operations (`nas`), the declared lead (`lead`), and the one file
//! that holds the records, `state/build/records.json` (`ONE`, and its brief `ONE_BRIEF`), which the
//! lead alone writes, whole, after each save on its own disk (docs/coordination.md steps 8b and
//! 8c); before the pool, today's files. Written once, generic over what is read of a snapshot
//! (`Snapshot`): the pool's whole records (agentcore::pool::records::Records, with the agent's
//! keys) or what an `Out` reads (`Recorded`). Writing them (saving, merging, taking them up) is the
//! pool's.

pub mod lead;
pub mod member;
pub mod nas;

use anyhow::{ensure, Context, Result};
use nas::Nas;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A member as the pool names it (§5).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    /// Its id, made once (member::member_id), named in everything it writes.
    pub id: String,
    /// Its host name: only a label (renaming a Mac, or macOS adding "-2" after a clash, changes
    /// nothing).
    pub host: String,
    /// The app it runs (§6.1, the app rule).
    pub app: String,
}

/// Whether `s` is a member id: `m-` and 16 lowercase hex digits.
pub fn is_member_id(s: &str) -> bool {
    s.strip_prefix("m-")
        .is_some_and(|h| h.len() == 16 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// The records before the pool, in today's layout: the manifest, the job keys, the unverified
/// uploads (a Mac's own build's, its jobs saving them: crate::out::Out::save_held). The pool's
/// first take-up starts from them.
pub const TODAY: [&str; 3] = ["state/build/manifest.json", "state/build/jobs.json", "state/build/pending.json"];

/// The records' one path (docs/coordination.md steps 8b and 8c): the lead's newest snapshot,
/// written whole after each save on its own disk. A reader reads it alone: one call.
pub const ONE: &str = "state/build/records.json";

/// The one path's brief (`Brief`): its snapshot's manifest and keys alone, written after it.
pub const ONE_BRIEF: &str = "state/build/brief.json";

/// What a reader reads of a snapshot.
pub trait Snapshot: Default + DeserializeOwned {
    /// Today's files read as the records (before the pool).
    fn today(nas: &dyn Nas) -> Result<Self>;
}

/// What a reader reads of a brief (a snapshot's manifest and keys alone, a quarter of its bytes:
/// written whole by the lead after each snapshot).
pub trait Brief: DeserializeOwned {
    /// Today's files' (before the pool).
    fn today(nas: &dyn Nas) -> Result<Self>;
}

/// Refuses to read today's files as the records when the pool has begun (a lead is declared:
/// lead::LEAD, never removed) and the one path can't be read now (a stale read: one name read as
/// missing for 40 minutes, task #630): today's files are weeks old once a lead saves the records,
/// and GC's live set would be theirs.
fn no_pool(nas: &dyn Nas) -> Result<()> {
    ensure!(!nas.exists(lead::LEAD)?, "the pool has begun and {ONE} can't be read now: the records aren't read");
    Ok(())
}

/// JSON file `p` parsed, or the default when there's none.
pub fn read_or_default<T: Default + DeserializeOwned>(nas: &dyn Nas, p: &str) -> Result<T> {
    match nas.read(p)? {
        Some(b) => serde_json::from_slice(&b).with_context(|| format!("parse {p}")),
        None => Ok(T::default()),
    }
}

/// JSON file `p` parsed; None when there's none.
fn read_json<T: DeserializeOwned>(nas: &dyn Nas, p: &str) -> Result<Option<T>> {
    let Some(b) = nas.read(p)? else { return Ok(None) };
    Ok(Some(serde_json::from_slice(&b).with_context(|| format!("parse {p}"))?))
}

/// The one path's snapshot (`ONE`) as read now; None when there's none (a pool whose lead never
/// saved, or a stale read): a lead's take-up (agentcore::pool::records::start).
pub fn load_one<S: Snapshot>(nas: &dyn Nas) -> Result<Option<S>> {
    read_json(nas, ONE)
}

/// The build's records as a reader has them now (§6.2): the one path's (`ONE`: one read); before
/// the pool, today's files (`no_pool`). A reader never reads today's files once a lead is declared.
pub fn now<S: Snapshot>(nas: &dyn Nas) -> Result<S> {
    match read_json(nas, ONE)? {
        Some(r) => Ok(r),
        None => {
            no_pool(nas)?;
            S::today(nas)
        }
    }
}

/// `now`'s brief (`B`): the one path's (`ONE_BRIEF`); today's files before the pool. One read pairs
/// keys and manifest of one version (invariant 4).
pub fn brief_now<B: Brief>(nas: &dyn Nas) -> Result<B> {
    match read_json(nas, ONE_BRIEF)? {
        Some(b) => Ok(b),
        None => {
            no_pool(nas)?;
            B::today(nas)
        }
    }
}

/// The parts of a records' brief a job's `Out` reads, its agent's local copy at `p`
/// (crate::out::BRIEF_ENV): the manifest; no unverified uploads nor sizes (a job that hands off
/// keeps none: crate::out::Out::open_with).
pub(crate) fn from_brief(p: &std::path::Path) -> Result<Recorded> {
    let b = std::fs::read(p).with_context(|| format!("read the records' brief its agent wrote, {}", p.display()))?;
    let l: Listed = serde_json::from_slice(&b).with_context(|| format!("parse {}", p.display()))?;
    Ok(Recorded { manifest: l.manifest, ..Default::default() })
}

/// The parts of the build's records an `Out` reads (crate::out::Out::open_with,
/// `recorded_sizes`).
#[derive(Default, Deserialize)]
pub(crate) struct Recorded {
    /// Logical name → content name.
    pub manifest: BTreeMap<String, String>,
    /// Uploads not yet verified on the NAS: content name → SHA-256.
    pub pending: BTreeMap<String, String>,
    /// The sizes of the files the manifest names (crate::out::SIZES).
    #[serde(default)]
    pub sizes: BTreeMap<String, u64>,
}

impl Snapshot for Recorded {
    /// The pending read before the manifest, as agentcore::pool::records::Records::today reads them.
    fn today(nas: &dyn Nas) -> Result<Recorded> {
        let pending = read_or_default(nas, TODAY[2])?;
        Ok(Recorded {
            manifest: read_or_default(nas, TODAY[0])?,
            pending,
            sizes: read_or_default(nas, crate::out::SIZES)?,
        })
    }
}

/// The manifest alone of a brief (`read_manifest`).
#[derive(Deserialize)]
struct Listed {
    manifest: BTreeMap<String, String>,
}

impl Brief for Listed {
    fn today(nas: &dyn Nas) -> Result<Listed> {
        Ok(Listed { manifest: read_or_default(nas, TODAY[0])? })
    }
}

/// The records' manifest as a reader has it now (`brief_now`: the one path's brief, a quarter of
/// its snapshot's bytes), read only: the accepted versions, for a reader that needs no `Out`.
pub fn read_manifest(nas: &dyn Nas) -> Result<BTreeMap<String, String>> {
    Ok(brief_now::<Listed>(nas)?.manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use store::nasfs::NasRoot;

    /// A NAS whose calls are counted (reads, stats).
    struct Counted<'a>(&'a NasRoot, std::cell::Cell<u64>);

    impl Nas for Counted<'_> {
        fn create_new(&self, path: &str, bytes: &[u8]) -> Result<nas::Created> {
            self.0.create_new(path, bytes)
        }
        fn write_whole(&self, path: &str, bytes: &[u8]) -> Result<()> {
            self.0.write_whole(path, bytes)
        }
        fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
            self.1.set(self.1.get() + 1);
            self.0.read(path)
        }
        fn exists(&self, path: &str) -> Result<bool> {
            self.1.set(self.1.get() + 1);
            self.0.exists(path)
        }
    }

    /// docs/coordination.md step 8c: a reader reads the one path alone, one NAS call (step 8b's
    /// two: the one path and a stat of the term after its own; before it five, the current term
    /// found and its snapshot read); before the pool, today's files; with the pool begun (a lead
    /// declared) and the one path not read (a stale read), never today's files.
    #[test]
    fn a_reader_reads_the_one_path_alone() {
        let d = tempfile::tempdir().unwrap();
        let root = NasRoot::local_for_test(d.path());
        let nas = Counted(&root, Default::default());
        let write = |p: &str, b: &str| root.write_whole(p, b.as_bytes()).unwrap();
        let calls = |f: &dyn Fn() -> String| {
            nas.1.set(0);
            (f(), nas.1.get())
        };
        let manifest = || now::<Recorded>(&nas).unwrap().manifest["a"].clone();
        let briefed = || read_manifest(&nas).unwrap()["a"].clone();
        // Before the pool: today's files.
        write(TODAY[0], r#"{"a": "a.1"}"#);
        write(TODAY[2], r#"{"a.1": "sha"}"#);
        assert_eq!(calls(&manifest).0, "a.1");
        assert_eq!(now::<Recorded>(&nas).unwrap().pending.len(), 1);
        assert_eq!(calls(&briefed).0, "a.1");
        // The pool begun, nothing saved yet (or a stale read of the one path): not read.
        write(lead::LEAD, r#"{"member": "m-000000000000000a"}"#);
        assert!(now::<Recorded>(&nas).is_err() && read_manifest(&nas).is_err());
        // Saved: the one path's, one call.
        write(ONE, r#"{"term":841,"seq":4,"manifest":{"a":"a.4"},"pending":{},"sizes":{"a.4":7}}"#);
        write(ONE_BRIEF, r#"{"term":841,"seq":4,"manifest":{"a":"a.4"},"keys":{}}"#);
        assert_eq!(calls(&manifest), ("a.4".into(), 1));
        assert_eq!(calls(&briefed), ("a.4".into(), 1));
        assert_eq!(now::<Recorded>(&nas).unwrap().sizes.get("a.4"), Some(&7));
    }
}
