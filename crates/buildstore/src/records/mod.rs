//! The build's records as a reader finds them (docs/pool.md §6.2; task #916 step 6a,
//! docs/crates.md §6): the NAS's operations (`nas`), the current term (`term`), and which file holds
//! the records now: the current term's snapshot or brief, or before the pool today's files. Written
//! once, generic over what is read of a snapshot (`Snapshot`): the pool's whole records
//! (pipeline::pool::records::Records, with the agent's keys) or what an `Out` reads (`Recorded`).
//! Writing them (saving, merging, taking a term up) is the pool's.

pub mod nas;
pub mod term;

use anyhow::{ensure, Context, Result};
use nas::Nas;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A member as the pool names it (§5).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    /// Its id, made once (pipeline::pool::member_id), named in everything it writes.
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

/// Term 1's records in today's layout: the manifest, the job keys, the unverified uploads.
pub const TODAY: [&str; 3] = ["state/build/manifest.json", "state/build/jobs.json", "state/build/pending.json"];

/// The path of term 1's first snapshot (pipeline::pool::records::first), which only its maker
/// writes.
pub const FIRST: &str = "state/build/term/1/first.json";

/// The path of term `term`'s snapshot.
pub fn path(term: u64) -> String {
    format!("state/build/term/{term}/records.json")
}

/// The path of term `term`'s brief (`BriefOf`): its snapshot's manifest and keys alone.
pub fn brief_path(term: u64) -> String {
    format!("state/build/term/{term}/brief.json")
}

/// What a reader reads of a term's snapshot.
pub trait Snapshot: Default + DeserializeOwned {
    /// The term it's of.
    fn term(&self) -> u64;
    /// Today's files read as term 1's records (numbered 0).
    fn today(nas: &dyn Nas) -> Result<Self>;
    /// Numbered `seq` in its term.
    fn with_seq(self, seq: u64) -> Self;
}

/// What a reader reads of a term's brief (a term after the first's snapshot's manifest and keys
/// alone, a quarter of its bytes: written whole by its lead after each snapshot), or of snapshot
/// `S` where there's none.
pub trait BriefOf<S>: DeserializeOwned {
    /// The term it's of.
    fn term(&self) -> u64;
    /// Snapshot `s`'s.
    fn of(s: S) -> Self;
    /// Today's files' (before the pool).
    fn today(nas: &dyn Nas) -> Result<Self>;
}

/// Refuses to read today's files as the records when the pool has begun (term 1's first snapshot
/// is there, never removed) but no term was found (term::current: the lead's hint, the terms'
/// listing and term 1 itself not read, stale): today's files may be days old once the lead stops
/// writing them, and GC's live set would be theirs.
fn no_pool(nas: &dyn Nas) -> Result<()> {
    ensure!(!nas.exists(FIRST)?, "the pool has begun and no term can be read now: the records aren't read");
    Ok(())
}

/// JSON file `p` parsed, or the default when there's none.
pub fn read_or_default<T: Default + DeserializeOwned>(nas: &dyn Nas, p: &str) -> Result<T> {
    match nas.read(p)? {
        Some(b) => serde_json::from_slice(&b).with_context(|| format!("parse {p}")),
        None => Ok(T::default()),
    }
}

/// Term `term`'s snapshot, as read now; None when it has none (its lead never saved one, or the
/// read is stale). An error when what's there can't be read now, or parsed.
pub fn load<S: Snapshot>(nas: &dyn Nas, term: u64) -> Result<Option<S>> {
    let Some(b) = nas.read(&path(term))? else { return Ok(None) };
    let r: S = serde_json::from_slice(&b).with_context(|| format!("parse {}", path(term)))?;
    ensure!(r.term() == term, "{} holds term {}'s records", path(term), r.term());
    Ok(Some(r))
}

/// The records a reader (a job, the map server, a catalogue) has while term `term` is current:
/// its snapshot, or while it has none yet (its lead taking it up, or gone before saving one)
/// the newest of a term before it; for term 1, as `term_one` says (`today` as it says). None
/// before any.
pub fn newest<S: Snapshot>(nas: &dyn Nas, term: u64, today: bool) -> Result<Option<S>> {
    for f in (2..=term).rev() {
        if let Some(r) = load(nas, f)? {
            return Ok(Some(r));
        }
    }
    if term == 0 {
        return Ok(None);
    }
    term_one(nas, today).map(Some)
}

/// The build's records as a reader has them now (§6.2): the current term's snapshot, or while
/// it has none the newest of a term before it (`newest`); before the pool's first term, today's
/// files (a Mac's own records, its jobs saving them: crate::out::Out::save_held). A reader
/// never reads today's files beside a term's snapshot: they're the lead's, for older apps'
/// readers, and stop once every member reads these (pipeline::agent::pool::Run::keep_today).
pub fn now<S: Snapshot>(nas: &dyn Nas) -> Result<S> {
    let cur = term::current(nas).context("the current term")?;
    if cur.term == 0 {
        no_pool(nas)?;
        return S::today(nas);
    }
    Ok(newest(nas, cur.term, true)?.unwrap_or_default())
}

/// `now`'s brief (`B`), reading a term's brief where it has one (its snapshot where it has none: a
/// lead on an app from before briefs, or one stopped between the two writes); today's files before
/// the pool. One read pairs keys and manifest of one version (invariant 4).
pub fn brief_now<S: Snapshot, B: BriefOf<S>>(nas: &dyn Nas) -> Result<B> {
    let cur = term::current(nas).context("the current term")?;
    if cur.term == 0 {
        no_pool(nas)?;
        return B::today(nas);
    }
    for f in (2..=cur.term).rev() {
        if let Some(b) = nas.read(&brief_path(f))? {
            let b: B = serde_json::from_slice(&b).with_context(|| format!("parse {}", brief_path(f)))?;
            ensure!(b.term() == f, "{} holds term {}'s brief", brief_path(f), b.term());
            return Ok(b);
        }
        if let Some(r) = load::<S>(nas, f)? {
            return Ok(B::of(r));
        }
    }
    Ok(B::of(term_one::<S>(nas, true)?))
}

/// Term 1's records as a take-up or a reader starts from them: its lead's newest snapshot; while it
/// has none, its first (`FIRST`); while that can't be read whole either (its maker stopped midway,
/// or its bytes landed out of order; or a stale read), today's three files once it has stayed so
/// longer than any stale read (`today`: the caller's to say), no lead having written them since
/// (term 1's writes them after its snapshot, and none is there), else `FirstUnreadable`.
pub fn term_one<S: Snapshot>(nas: &dyn Nas, today: bool) -> Result<S> {
    if let Some(r) = load(nas, 1)? {
        return Ok(r);
    }
    match nas.read(FIRST)?.map(|b| serde_json::from_slice::<S>(&b)) {
        Some(Ok(r)) if r.term() == 1 => Ok(r),
        _ if today => Ok(S::today(nas)?.with_seq(1)),
        _ => Err(FirstUnreadable.into()),
    }
}

/// `term_one`'s error while term 1 has no snapshot that reads whole: tried again, until it has
/// stayed so longer than any stale read (`today`).
#[derive(Debug)]
pub struct FirstUnreadable;

impl std::fmt::Display for FirstUnreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "term 1's first records can't be read whole yet: try again shortly")
    }
}

impl std::error::Error for FirstUnreadable {}

/// The parts of the build's records an `Out` reads (crate::out::Out::open_with,
/// `recorded_sizes`).
#[derive(Default, Deserialize)]
pub struct Recorded {
    /// The term they're of.
    pub term: u64,
    /// Logical name → content name.
    pub manifest: BTreeMap<String, String>,
    /// Uploads not yet verified on the NAS: content name → SHA-256.
    pub pending: BTreeMap<String, String>,
    /// The sizes of the files the manifest names (crate::out::SIZES).
    #[serde(default)]
    pub sizes: BTreeMap<String, u64>,
}

impl Snapshot for Recorded {
    fn term(&self) -> u64 {
        self.term
    }

    /// The pending read before the manifest, as pipeline::pool::records::Records::today reads them.
    fn today(nas: &dyn Nas) -> Result<Recorded> {
        let pending = read_or_default(nas, TODAY[2])?;
        Ok(Recorded {
            term: 1,
            manifest: read_or_default(nas, TODAY[0])?,
            pending,
            sizes: read_or_default(nas, crate::out::SIZES)?,
        })
    }

    fn with_seq(self, _: u64) -> Self {
        self
    }
}

/// The manifest alone of a brief or a snapshot (`read_manifest`).
#[derive(Deserialize)]
struct Listed {
    term: u64,
    manifest: BTreeMap<String, String>,
}

impl BriefOf<Recorded> for Listed {
    fn term(&self) -> u64 {
        self.term
    }

    fn of(r: Recorded) -> Listed {
        Listed { term: r.term, manifest: r.manifest }
    }

    fn today(nas: &dyn Nas) -> Result<Listed> {
        Ok(Listed { term: 1, manifest: read_or_default(nas, TODAY[0])? })
    }
}

/// The records' manifest as a reader has it now (`brief_now`: the current term's brief, a quarter of
/// its snapshot's bytes), read only: the accepted versions, for a reader that needs no `Out`.
pub fn read_manifest(nas: &dyn Nas) -> Result<BTreeMap<String, String>> {
    Ok(brief_now::<Recorded, Listed>(nas)?.manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use store::nasfs::NasRoot;

    /// A reader finds the current term's brief, else its snapshot, else today's files before the
    /// pool: the manifest and what an `Out` reads of the snapshot alike.
    #[test]
    fn a_reader_finds_the_current_terms_records() {
        let d = tempfile::tempdir().unwrap();
        let nas = NasRoot::local_for_test(d.path());
        let write = |p: &str, b: &str| {
            let p = d.path().join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b).unwrap();
        };
        write(TODAY[0], r#"{"a": "a.1"}"#);
        write(TODAY[2], r#"{"a.1": "sha"}"#);
        assert_eq!(read_manifest(&nas).unwrap().len(), 1);
        let r: Recorded = now(&nas).unwrap();
        assert_eq!((r.term, r.manifest.len(), r.pending.len()), (1, 1, 1));

        let term = |e: u64| {
            format!(
                r#"{{"term":{e},"member":"m-000000000000000a","host":"h","app":"x","since":1,"how":"h","from":{}}}"#,
                e - 1
            )
        };
        write(&term::path(1), &term(1));
        write(&term::path(2), &term(2));
        write(FIRST, r#"{"term":1,"seq":1,"manifest":{},"keys":{},"pending":{}}"#);
        write(
            &path(2),
            r#"{"term":2,"seq":3,"manifest":{"b":"b.1","c":"c.1"},"keys":{"x":1},"pending":{},"sizes":{"b.1":7},"reflected":[]}"#,
        );
        let r: Recorded = now(&nas).unwrap();
        assert_eq!((r.term, r.manifest.len(), r.sizes.get("b.1")), (2, 2, Some(&7)));
        assert_eq!(read_manifest(&nas).unwrap().len(), 2, "its snapshot, with no brief");
        write(&brief_path(2), r#"{"term":2,"seq":3,"manifest":{"b":"b.1"},"keys":{}}"#);
        assert_eq!(read_manifest(&nas).unwrap().len(), 1, "its brief");
        write(&brief_path(2), r#"{"term":1,"seq":3,"manifest":{},"keys":{}}"#);
        assert!(read_manifest(&nas).is_err(), "another term's");
    }
}
